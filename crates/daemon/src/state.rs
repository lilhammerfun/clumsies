use std::sync::Arc;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use sqlx::{Row, SqlitePool};
use tokio::sync::{Mutex, Notify, Semaphore};
use tokio::task::JoinHandle;
use uuid::Uuid;

use super::*;
use crate::draft::daemon_draft_scope_from_str;

const STARTUP_CREDENTIAL_LOAD_TIMEOUT: Duration = Duration::from_secs(10);
const LAZY_CREDENTIAL_LOAD_TIMEOUT: Duration = Duration::from_secs(5);
const CREDENTIAL_RECOVERY_COOLDOWN: Duration = Duration::from_secs(10);

const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const SERVER_RESPONSE_CACHE_MAX_BODY_BYTES: usize = 1024 * 1024;
const SERVER_RESPONSE_CACHE_MAX_PENDING_WRITES: usize = 16;

fn build_http_client(
    connect_timeout: Duration,
    request_timeout: Duration,
) -> Result<reqwest::Client, DaemonError> {
    reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(request_timeout)
        .build()
        .map_err(DaemonError::Reqwest)
}

fn legacy_project_memory_read_only_error(resource_id: &str) -> DaemonError {
    DaemonError::InvalidRequest(format!(
        "legacy Project Memory {resource_id} is read-only; MCP mutations may target only selected Organization Memory"
    ))
}

fn project_memory_authority_removed_error() -> DaemonError {
    DaemonError::InvalidRequest(
        "Project is a Draft carrier, not a Memory authority scope; store an Organization proposal"
            .to_owned(),
    )
}

#[derive(Default)]
pub(crate) struct CredentialRecovery {
    last_attempt: Option<Instant>,
}

pub(crate) struct RuntimeProjectConfigSnapshot {
    pub(crate) config: RuntimeProjectConfig,
    pub(crate) session_revision: u64,
}

struct RuntimeProjectConfigState {
    config: RuntimeProjectConfig,
    session_revision: u64,
}

type ServerResponseCacheKey = (String, String);

#[derive(Default)]
struct ServerResponseCacheKeyState {
    in_flight: usize,
    latest_success_revision: Option<u64>,
}

struct PendingServerResponseCacheWrite {
    revision: u64,
    response: DaemonServerResponse,
}

struct ServerResponseCacheRequest {
    inner: Arc<DaemonInner>,
    key: ServerResponseCacheKey,
    revision: u64,
}

struct ServerResponseCacheTransition<'a> {
    inner: &'a DaemonInner,
    _write: tokio::sync::MutexGuard<'a, ()>,
    published: bool,
}

impl ServerResponseCacheRequest {
    fn server_url(&self) -> &str {
        &self.key.0
    }
}

impl Drop for ServerResponseCacheRequest {
    fn drop(&mut self) {
        self.inner
            .server_response_cache
            .lock()
            .expect("server response cache mutex poisoned")
            .finish_request(&self.key);
    }
}

/// Coalesces successful GETs for one bounded background SQLite writer.
/// Request state lives only until its request and queued write both finish.
#[derive(Default)]
struct ServerResponseCacheState {
    next_revision: u64,
    minimum_revision: u64,
    keys: BTreeMap<ServerResponseCacheKey, ServerResponseCacheKeyState>,
    pending: BTreeMap<ServerResponseCacheKey, PendingServerResponseCacheWrite>,
    active: Option<(ServerResponseCacheKey, u64)>,
    worker_running: bool,
}

impl ServerResponseCacheState {
    fn invalidate(&mut self) {
        self.minimum_revision = self.next_revision;
        self.pending.clear();
        for state in self.keys.values_mut() {
            state.latest_success_revision = None;
        }
        let active_key = self.active.as_ref().map(|(key, _)| key.clone());
        self.keys.retain(|key, state| {
            state.in_flight > 0 || active_key.as_ref().is_some_and(|active| active == key)
        });
    }

    fn finish_request(&mut self, key: &ServerResponseCacheKey) {
        let state = self
            .keys
            .get_mut(key)
            .expect("active cache request must retain its key state");
        state.in_flight = state
            .in_flight
            .checked_sub(1)
            .expect("cache request count underflow");
        self.remove_key_if_idle(key);
    }

    fn remove_key_if_idle(&mut self, key: &ServerResponseCacheKey) {
        let idle = self.keys.get(key).is_some_and(|state| state.in_flight == 0)
            && !self.pending.contains_key(key)
            && self.active.as_ref().is_none_or(|(active, _)| active != key);
        if idle {
            self.keys.remove(key);
        }
    }
}

#[derive(Clone)]
pub struct DaemonState {
    pub(crate) inner: Arc<DaemonInner>,
}

pub(crate) struct DaemonInner {
    pub(crate) config: DaemonConfig,
    project_config: RwLock<RuntimeProjectConfigState>,
    pub(crate) project_config_mutation: Mutex<()>,
    pub(crate) credential_store: Arc<dyn CredentialStore>,
    pub(crate) pool: SqlitePool,
    pub(crate) http: reqwest::Client,
    pub(crate) daemon_installation_id: String,
    pub(crate) sync_notify: Notify,
    pub(crate) sync_lock: Mutex<()>,
    pub(crate) commit_sync_run_scope: RwLock<commit_sync::CommitSyncRunScope>,
    pub(crate) token_refresh: Mutex<()>,
    server_response_cache: std::sync::Mutex<ServerResponseCacheState>,
    server_response_cache_write: Mutex<()>,
    pub(crate) credential_recovery: Mutex<CredentialRecovery>,
    pub(crate) search_models: Arc<dyn search::models::SearchModels>,
    pub(crate) search_model_gate: Arc<Semaphore>,
    pub(crate) search_index_notify: Notify,
    pub(crate) search_lock: Mutex<()>,
    pub(crate) retrieval_history_lock: Mutex<()>,
    pub(crate) draft_mutation_lock: Mutex<()>,
    pub(crate) local_setup_lock: Mutex<()>,
    pub(crate) storage_access: tokio::sync::RwLock<()>,
}

impl DaemonInner {
    fn project_config_snapshot(&self) -> RuntimeProjectConfigSnapshot {
        let state = self
            .project_config
            .read()
            .expect("project config rwlock poisoned");
        RuntimeProjectConfigSnapshot {
            config: state.config.clone(),
            session_revision: state.session_revision,
        }
    }

    fn publish_project_config(&self, project_config: RuntimeProjectConfig, new_session: bool) {
        let mut state = self
            .project_config
            .write()
            .expect("project config rwlock poisoned");
        if new_session {
            state.session_revision = state
                .session_revision
                .checked_add(1)
                .expect("Server session revision exhausted");
        }
        state.config = project_config;
    }
}

impl ServerResponseCacheTransition<'_> {
    fn publish(mut self, project_config: RuntimeProjectConfig) {
        let mut cache = self
            .inner
            .server_response_cache
            .lock()
            .expect("server response cache mutex poisoned");
        self.inner.publish_project_config(project_config, true);
        cache.invalidate();
        self.published = true;
    }
}

impl Drop for ServerResponseCacheTransition<'_> {
    fn drop(&mut self) {
        if !self.published {
            self.inner
                .server_response_cache
                .lock()
                .expect("server response cache mutex poisoned")
                .invalidate();
        }
    }
}

impl DaemonState {
    pub async fn initialize(config: DaemonConfig) -> Result<Self, DaemonError> {
        let credential_store =
            SystemCredentialStore::new(config.keychain_service.clone(), KEYCHAIN_ACCOUNT);
        Self::initialize_with_credential_store(config, Arc::new(credential_store)).await
    }

    pub async fn initialize_with_credential_store(
        config: DaemonConfig,
        credential_store: Arc<dyn CredentialStore>,
    ) -> Result<Self, DaemonError> {
        let search_models = search::production_models(config.cache_dir.clone());
        Self::initialize_with_credential_store_and_search_models(
            config,
            credential_store,
            search_models,
        )
        .await
    }

    pub(crate) async fn initialize_with_credential_store_and_search_models(
        config: DaemonConfig,
        credential_store: Arc<dyn CredentialStore>,
        search_models: Arc<dyn search::models::SearchModels>,
    ) -> Result<Self, DaemonError> {
        prepare_directories(&config)?;
        let pool = connect_local_db(&config.local_db_path()).await?;
        migrate_local_db(&pool).await?;
        // A Coding Agent integration spans repository files and the local
        // ownership manifest. Recover prepared filesystem transactions before
        // normal reconciliation. A conflict is isolated to that Adapter: the
        // resident daemon stays available so Desktop can diagnose it and
        // already-running Agents in unrelated repositories keep working.
        if let Err(error) = agent_adapter::recover_pending_fs_ops(&pool).await {
            tracing::error!("adapter filesystem recovery is pending: {error}");
        }
        reset_memory_cache_if_required(&pool, &config.cache_dir).await?;
        recover_interrupted_operations(&pool).await?;
        retrieval_history::recover_interrupted_runs(&pool).await?;
        let daemon_installation_id = load_or_create_installation_id(&pool).await?;
        let credentials =
            load_startup_credentials(credential_store.clone(), STARTUP_CREDENTIAL_LOAD_TIMEOUT)
                .await;
        let project_config = load_project_config(&pool, &config.project, credentials).await?;
        let http = build_http_client(HTTP_CONNECT_TIMEOUT, HTTP_REQUEST_TIMEOUT)?;

        let state = Self {
            inner: Arc::new(DaemonInner {
                config,
                project_config: RwLock::new(RuntimeProjectConfigState {
                    config: project_config,
                    session_revision: 0,
                }),
                project_config_mutation: Mutex::new(()),
                credential_store,
                pool,
                http,
                daemon_installation_id,
                sync_notify: Notify::new(),
                sync_lock: Mutex::new(()),
                commit_sync_run_scope: RwLock::new(commit_sync::CommitSyncRunScope::Idle),
                token_refresh: Mutex::new(()),
                server_response_cache: std::sync::Mutex::new(ServerResponseCacheState::default()),
                server_response_cache_write: Mutex::new(()),
                credential_recovery: Mutex::new(CredentialRecovery::default()),
                search_models,
                search_model_gate: Arc::new(Semaphore::new(1)),
                search_index_notify: Notify::new(),
                search_lock: Mutex::new(()),
                retrieval_history_lock: Mutex::new(()),
                draft_mutation_lock: Mutex::new(()),
                local_setup_lock: Mutex::new(()),
                storage_access: tokio::sync::RwLock::new(()),
            }),
        };
        project_storage::resume_pending_moves(&state);
        Ok(state)
    }

    pub fn local_db_path(&self) -> PathBuf {
        self.inner.config.local_db_path()
    }

    pub fn daemon_installation_id(&self) -> &str {
        &self.inner.daemon_installation_id
    }

    pub fn project_config_status(&self) -> DaemonProjectConfig {
        self.project_config_view()
    }

    pub(crate) fn project_config(&self) -> RuntimeProjectConfig {
        self.project_config_snapshot().config
    }

    pub(crate) fn project_config_snapshot(&self) -> RuntimeProjectConfigSnapshot {
        self.inner.project_config_snapshot()
    }

    pub(crate) fn publish_project_config(&self, project_config: RuntimeProjectConfig) {
        self.inner.publish_project_config(project_config, false);
    }

    #[cfg(test)]
    pub(crate) async fn project_config_with_credentials(&self) -> RuntimeProjectConfig {
        self.project_config_with_credentials_snapshot().await.config
    }

    pub(crate) async fn project_config_with_credentials_snapshot(
        &self,
    ) -> RuntimeProjectConfigSnapshot {
        let snapshot = self.project_config_snapshot();
        if snapshot.config.access_token.is_some() {
            return snapshot;
        }
        {
            let mut recovery = self.inner.credential_recovery.lock().await;
            if recovery
                .last_attempt
                .is_some_and(|attempt| attempt.elapsed() < CREDENTIAL_RECOVERY_COOLDOWN)
            {
                return self.project_config_snapshot();
            }
            recovery.last_attempt = Some(Instant::now());
        }
        let loaded = tokio::time::timeout(
            LAZY_CREDENTIAL_LOAD_TIMEOUT,
            load_server_credentials(self.inner.credential_store.clone()),
        )
        .await;
        let Ok(Ok(Some(credentials))) = loaded else {
            return self.project_config_snapshot();
        };

        let _mutation = self.inner.project_config_mutation.lock().await;
        let current = self.project_config_snapshot();
        if current.session_revision != snapshot.session_revision
            || current.config.server_url != snapshot.config.server_url
            || current.config.access_token.is_some()
            || credentials.server_url != current.config.server_url
        {
            return current;
        }
        let transition = match self.begin_server_response_cache_transition().await {
            Ok(transition) => transition,
            Err(error) => {
                tracing::warn!(
                    "failed to clear Server response cache before credential recovery: {error}"
                );
                return current;
            }
        };
        let mut recovered = current.config;
        recovered.access_token = Some(credentials.access_token);
        recovered.refresh_token = credentials.refresh_token;
        transition.publish(recovered);
        tracing::info!("recovered Server session from the credential store");
        self.project_config_snapshot()
    }

    pub async fn replace_project_config(
        &self,
        request: DaemonProjectConfigUpdateRequest,
    ) -> Result<DaemonProjectConfig, DaemonError> {
        let project_config = RuntimeProjectConfig {
            server_url: request.server_url.trim().to_owned(),
            project_id: request.project_id.and_then(non_empty_string),
            memory_guidelines_path: request.memory_guidelines_path.and_then(non_empty_string),
            access_token: request.access_token.and_then(non_empty_string),
            refresh_token: request.refresh_token.and_then(non_empty_string),
        };
        project_config.validate()?;
        let _mutation = self.inner.project_config_mutation.lock().await;
        let transition = self.begin_server_response_cache_transition().await?;
        let previous_credentials = self.project_config().credentials();
        replace_server_credentials(
            self.inner.credential_store.clone(),
            project_config.credentials(),
        )
        .await?;
        if let Err(error) =
            save_project_metadata(&self.inner.pool, &project_config.metadata()).await
        {
            if let Err(rollback_error) = replace_server_credentials(
                self.inner.credential_store.clone(),
                previous_credentials,
            )
            .await
            {
                return Err(DaemonError::CredentialStore(CredentialStoreError::new(
                    format!(
                        "failed to persist project metadata ({error}) and failed to restore the previous Keychain session ({rollback_error})"
                    ),
                )));
            }
            return Err(error);
        }
        transition.publish(project_config);
        if let Err(error) = queue_retrying_operations(&self.inner.pool).await {
            tracing::warn!(
                "failed to requeue retrying draft operations after project config commit: {error}"
            );
        }
        self.request_sync();
        Ok(self.project_config_view())
    }

    pub(crate) async fn clear_server_tokens_if_current(
        &self,
        expected_session_revision: u64,
        expected_server_url: &str,
        expected_access_token: &str,
    ) -> Result<bool, DaemonError> {
        let _mutation = self.inner.project_config_mutation.lock().await;
        let current = self.project_config_snapshot();
        if current.session_revision != expected_session_revision
            || current.config.server_url != expected_server_url
            || current.config.access_token.as_deref() != Some(expected_access_token)
        {
            return Ok(false);
        }
        let transition = self.begin_server_response_cache_transition().await?;
        replace_server_credentials(self.inner.credential_store.clone(), None).await?;
        let mut project_config = current.config;
        project_config.access_token = None;
        project_config.refresh_token = None;
        transition.publish(project_config);
        Ok(true)
    }

    pub async fn select_project(
        &self,
        request: DaemonProjectSelectionRequest,
    ) -> Result<DaemonProjectConfig, DaemonError> {
        let project_id = non_empty_string(request.project_id).ok_or_else(|| {
            DaemonError::InvalidRequest("project_id must not be empty".to_owned())
        })?;
        let _mutation = self.inner.project_config_mutation.lock().await;
        let mut project_config = self.project_config();
        project_config.project_id = Some(project_id);
        project_config.validate()?;
        save_project_metadata(&self.inner.pool, &project_config.metadata()).await?;
        self.publish_project_config(project_config);
        self.request_sync();
        Ok(self.project_config_view())
    }

    pub async fn resolve_project_binding(
        &self,
        request: DaemonProjectBindingResolveRequest,
    ) -> Result<DaemonProjectBinding, DaemonError> {
        let required_adapter = request.required_adapter;
        let workspace_path = canonical_workspace_directory(&request.workspace_path)?;
        let server_url = canonical_server_url(&self.project_config().server_url)?;
        let rows = sqlx::query(
            "SELECT server_url, workspace_root, project_id, revision, created_at, updated_at
             FROM project_bindings
             WHERE server_url = $1",
        )
        .bind(&server_url)
        .fetch_all(&self.inner.pool)
        .await?;

        let mut candidates = vec![workspace_path.clone()];
        // A git worktree belongs to the same repository as its main checkout,
        // so it should resolve to the same Project. Fall back to the main
        // repository root when the worktree path itself is not bound.
        if let Some(main_root) = git_worktree_main_root(&workspace_path)
            && main_root != workspace_path
        {
            candidates.push(main_root);
        }

        let mut best: Option<(usize, DaemonProjectBinding)> = None;
        for candidate in &candidates {
            for row in &rows {
                let binding = project_binding_from_row(row)?;
                // Stored roots were canonicalized at insert time; re-canonicalize
                // them now so a later workspace move or symlink change (e.g. a
                // repository migrated to an external volume) still matches.
                let root = canonical_binding_root(&binding.workspace_root);
                if !candidate.starts_with(&root) {
                    continue;
                }
                let specificity = root.components().count();
                if best
                    .as_ref()
                    .is_some_and(|(best_specificity, _)| *best_specificity >= specificity)
                {
                    continue;
                }
                best = Some((specificity, binding));
            }
        }

        let binding = best
            .map(|(_, binding)| binding)
            .ok_or_else(|| DaemonError::State {
                code: "project_binding_not_found",
                message: format!(
                    "No Project is bound to workspace path {} on {server_url}",
                    workspace_path.display()
                ),
            })?;
        if let Some(required) = required_adapter {
            agent_adapter::require_runtime_delivery(self, &binding, required).await?;
        }
        Ok(binding)
    }

    pub async fn list_project_bindings(
        &self,
        request: DaemonProjectBindingListRequest,
    ) -> Result<DaemonProjectBindingListResponse, DaemonError> {
        let project_id = non_empty_string(request.project_id).ok_or_else(|| {
            DaemonError::InvalidRequest("project_id must not be empty".to_owned())
        })?;
        let server_url = canonical_server_url(&self.project_config().server_url)?;
        let rows = sqlx::query(
            "SELECT server_url, workspace_root, project_id, revision, created_at, updated_at
             FROM project_bindings
             WHERE server_url = $1 AND project_id = $2
             ORDER BY workspace_root",
        )
        .bind(server_url)
        .bind(project_id)
        .fetch_all(&self.inner.pool)
        .await?;
        Ok(DaemonProjectBindingListResponse {
            items: rows
                .iter()
                .map(project_binding_from_row)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    pub async fn replace_project_binding(
        &self,
        request: DaemonProjectBindingReplaceRequest,
    ) -> Result<DaemonProjectBinding, DaemonError> {
        let workspace_root = canonical_workspace_directory(&request.workspace_root)?;
        let project_id = non_empty_string(request.project_id).ok_or_else(|| {
            DaemonError::InvalidRequest("project_id must not be empty".to_owned())
        })?;
        commit_sync::validate_cache_component("project_id", &project_id)?;
        let server_url = canonical_server_url(&self.project_config().server_url)?;

        let response = execute_authenticated_server_request(
            self,
            reqwest::Method::GET,
            &format!("/api/v1/projects/{project_id}"),
            &BTreeMap::new(),
            None,
        )
        .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND
            || response.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(DaemonError::State {
                code: "project_binding_unresolved",
                message: format!(
                    "Project {project_id} does not exist or is not accessible on {server_url}"
                ),
            });
        }
        ensure_server_success(response).await?;

        let _guard = self.inner.local_setup_lock.lock().await;
        agent_adapter::recover_pending_fs_ops_for_workspace(
            &self.inner.pool,
            &server_url,
            &workspace_root,
        )
        .await?;
        let workspace_root = workspace_root.display().to_string();
        let mut tx = self.inner.pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing = sqlx::query(
            "SELECT server_url, workspace_root, project_id, revision, created_at, updated_at
             FROM project_bindings
             WHERE server_url = $1 AND workspace_root = $2",
        )
        .bind(&server_url)
        .bind(&workspace_root)
        .fetch_optional(&mut *tx)
        .await?;

        if let Some(row) = existing {
            let existing = project_binding_from_row(&row)?;
            if request.expected_revision.is_none() && existing.project_id == project_id {
                tx.commit().await?;
                return Ok(existing);
            }
            if request.expected_revision != Some(existing.revision) {
                return Err(DaemonError::State {
                    code: "project_binding_changed",
                    message: format!(
                        "Project binding for {workspace_root} changed from the expected revision"
                    ),
                });
            }
            sqlx::query(
                "UPDATE project_bindings
                 SET project_id = $3, revision = revision + 1,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE server_url = $1 AND workspace_root = $2",
            )
            .bind(&server_url)
            .bind(&workspace_root)
            .bind(&project_id)
            .execute(&mut *tx)
            .await?;
        } else {
            if request.expected_revision.is_some() {
                return Err(DaemonError::State {
                    code: "project_binding_changed",
                    message: format!("Project binding for {workspace_root} no longer exists"),
                });
            }
            sqlx::query(
                "INSERT INTO project_bindings (server_url, workspace_root, project_id, revision)
                 VALUES ($1, $2, $3, 1)",
            )
            .bind(&server_url)
            .bind(&workspace_root)
            .bind(&project_id)
            .execute(&mut *tx)
            .await?;
        }

        let row = sqlx::query(
            "SELECT server_url, workspace_root, project_id, revision, created_at, updated_at
             FROM project_bindings
             WHERE server_url = $1 AND workspace_root = $2",
        )
        .bind(&server_url)
        .bind(&workspace_root)
        .fetch_one(&mut *tx)
        .await?;
        let binding = project_binding_from_row(&row)?;
        tx.commit().await?;
        self.request_sync();
        Ok(binding)
    }

    pub async fn remove_project_binding(
        &self,
        request: DaemonProjectBindingRemoveRequest,
    ) -> Result<DaemonProjectBindingRemoveResponse, DaemonError> {
        let workspace_root = canonical_workspace_directory(&request.workspace_root)?;
        let server_url = canonical_server_url(&self.project_config().server_url)?;
        let _guard = self.inner.local_setup_lock.lock().await;
        agent_adapter::recover_pending_fs_ops_for_workspace(
            &self.inner.pool,
            &server_url,
            &workspace_root,
        )
        .await?;
        let workspace_root = workspace_root.display().to_string();
        let adapter_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)
             FROM project_agent_adapters
             WHERE server_url = $1 AND workspace_root = $2",
        )
        .bind(&server_url)
        .bind(&workspace_root)
        .fetch_one(&self.inner.pool)
        .await?;
        if adapter_count > 0 {
            return Err(DaemonError::State {
                code: "project_binding_has_adapters",
                message: "Remove the repository's Coding Agent integrations before unbinding it."
                    .to_owned(),
            });
        }
        let result = sqlx::query(
            "DELETE FROM project_bindings
             WHERE server_url = $1 AND workspace_root = $2 AND revision = $3",
        )
        .bind(&server_url)
        .bind(&workspace_root)
        .bind(request.expected_revision)
        .execute(&self.inner.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(DaemonError::State {
                code: "project_binding_changed",
                message: format!(
                    "Project binding for {workspace_root} changed from the expected revision"
                ),
            });
        }
        Ok(DaemonProjectBindingRemoveResponse {
            workspace_root,
            removed: true,
        })
    }

    pub async fn list_project_agent_adapters(
        &self,
        request: DaemonProjectAgentAdapterListRequest,
    ) -> Result<DaemonProjectAgentAdapterListResponse, DaemonError> {
        agent_adapter::list(self, request).await
    }

    pub async fn list_all_project_agent_adapters(
        &self,
    ) -> Result<DaemonProjectAgentAdapterListResponse, DaemonError> {
        agent_adapter::list_all(self).await
    }

    pub async fn inspect_legacy_agent_adapters(
        &self,
        request: DaemonLegacyAgentAdapterInspectionRequest,
    ) -> Result<DaemonLegacyAgentAdapterInspectionResponse, DaemonError> {
        agent_adapter::inspect_legacy(self, request).await
    }

    pub async fn agent_adapter_settings(
        &self,
    ) -> Result<crate::DaemonAgentAdapterSettings, DaemonError> {
        agent_adapter::global::list(self).await
    }

    pub async fn set_agent_adapter(
        &self,
        request: crate::DaemonSetAgentAdapterRequest,
    ) -> Result<crate::DaemonAgentAdapterSettings, DaemonError> {
        agent_adapter::global::set(self, request).await
    }

    pub async fn inspect_codex_plugin(
        &self,
        request: DaemonCodexPluginRequest,
    ) -> Result<DaemonCodexPluginStatus, DaemonError> {
        agent_adapter::inspect_codex_plugin(self, request).await
    }

    pub async fn reconcile_codex_plugin(
        &self,
        request: DaemonCodexPluginRequest,
    ) -> Result<DaemonCodexPluginStatus, DaemonError> {
        agent_adapter::reconcile_codex_plugin(self, request).await
    }

    pub async fn install_project_agent_adapter(
        &self,
        request: DaemonProjectAgentAdapterInstallRequest,
    ) -> Result<DaemonProjectAgentAdapter, DaemonError> {
        agent_adapter::install(self, request).await
    }

    pub async fn remove_project_agent_adapter(
        &self,
        request: DaemonProjectAgentAdapterRemoveRequest,
    ) -> Result<DaemonProjectAgentAdapterRemoveResponse, DaemonError> {
        agent_adapter::remove(self, request).await
    }

    pub async fn server_request(
        &self,
        request: DaemonServerRequest,
    ) -> Result<DaemonServerResponse, DaemonError> {
        validate_server_proxy_path(&request.path)?;
        if request
            .body
            .as_ref()
            .is_some_and(|body| body.len() > 10 * 1024 * 1024)
        {
            return Err(DaemonError::InvalidRequest(
                "server request body exceeds 10 MiB".to_owned(),
            ));
        }
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|_| DaemonError::InvalidRequest("invalid server request method".to_owned()))?;
        if ![
            reqwest::Method::GET,
            reqwest::Method::POST,
            reqwest::Method::PUT,
            reqwest::Method::PATCH,
            reqwest::Method::DELETE,
        ]
        .contains(&method)
        {
            return Err(DaemonError::InvalidRequest(
                "server request method is not allowed".to_owned(),
            ));
        }
        let headers = filter_proxy_request_headers(request.headers);
        let cacheable = method == reqwest::Method::GET;
        let cache_request =
            cacheable.then(|| self.start_server_response_cache_request(&request.path));
        let response = match execute_authenticated_server_request(
            self,
            method,
            &request.path,
            &headers,
            request.body.map(String::into_bytes),
        )
        .await
        {
            Ok(response) => response,
            Err(error) if cacheable && error.is_retryable() => {
                if let Some(cached) = load_cached_server_response(
                    &self.inner.pool,
                    cache_request
                        .as_ref()
                        .expect("cacheable request must retain cache state")
                        .server_url(),
                    &request.path,
                )
                .await?
                {
                    tracing::warn!(event = "http_cache_fallback", request_id = %crate::diagnostics::request_id(), route = %crate::diagnostics::route(&request.path));
                    return Ok(cached);
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        let status = response.status().as_u16();
        let headers = filter_proxy_response_headers(response.headers());
        let body = match response
            .text()
            .await
            .map_err(|error| crate::diagnostics::http_error(error, "body"))
        {
            Ok(body) => body,
            Err(error) if cacheable => {
                if let Some(cached) = load_cached_server_response(
                    &self.inner.pool,
                    cache_request
                        .as_ref()
                        .expect("cacheable request must retain cache state")
                        .server_url(),
                    &request.path,
                )
                .await?
                {
                    tracing::warn!(event = "http_cache_fallback", request_id = %crate::diagnostics::request_id(), route = %crate::diagnostics::route(&request.path));
                    return Ok(cached);
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        let response = DaemonServerResponse {
            status,
            headers,
            body,
        };
        if cacheable && ((200..300).contains(&status) || status == 404) {
            self.queue_server_response_cache_write(
                cache_request
                    .as_ref()
                    .expect("cacheable request must retain cache state"),
                &response,
            );
        } else if cacheable
            && is_retryable_http_status(status)
            && let Some(cached) = load_cached_server_response(
                &self.inner.pool,
                cache_request
                    .as_ref()
                    .expect("cacheable request must retain cache state")
                    .server_url(),
                &request.path,
            )
            .await?
        {
            tracing::warn!(event = "http_cache_fallback", request_id = %crate::diagnostics::request_id(), route = %crate::diagnostics::route(&request.path));
            return Ok(cached);
        }
        Ok(response)
    }

    fn start_server_response_cache_request(&self, path: &str) -> ServerResponseCacheRequest {
        let mut cache = self
            .inner
            .server_response_cache
            .lock()
            .expect("server response cache mutex poisoned");
        let revision = cache.next_revision;
        cache.next_revision = cache
            .next_revision
            .checked_add(1)
            .expect("server response cache revision exhausted");
        let server_url = self.project_config().server_url;
        let key = (server_url, path.to_owned());
        let key_state = cache.keys.entry(key.clone()).or_default();
        key_state.in_flight = key_state
            .in_flight
            .checked_add(1)
            .expect("cache request count exhausted");
        ServerResponseCacheRequest {
            inner: self.inner.clone(),
            key,
            revision,
        }
    }

    fn queue_server_response_cache_write(
        &self,
        request: &ServerResponseCacheRequest,
        response: &DaemonServerResponse,
    ) {
        let should_copy = {
            let mut cache = self
                .inner
                .server_response_cache
                .lock()
                .expect("server response cache mutex poisoned");
            if request.revision < cache.minimum_revision {
                return;
            }
            let key_state = cache
                .keys
                .get_mut(&request.key)
                .expect("active cache request must retain its key state");
            if key_state
                .latest_success_revision
                .is_some_and(|latest| latest >= request.revision)
            {
                false
            } else {
                key_state.latest_success_revision = Some(request.revision);
                true
            }
        };
        if !should_copy || response.body.len() > SERVER_RESPONSE_CACHE_MAX_BODY_BYTES {
            return;
        }
        let response = response.clone();
        let start_worker = {
            let mut cache = self
                .inner
                .server_response_cache
                .lock()
                .expect("server response cache mutex poisoned");
            if request.revision < cache.minimum_revision
                || cache
                    .keys
                    .get(&request.key)
                    .and_then(|state| state.latest_success_revision)
                    != Some(request.revision)
            {
                return;
            }
            if !cache.pending.contains_key(&request.key)
                && cache.pending.len() == SERVER_RESPONSE_CACHE_MAX_PENDING_WRITES
            {
                let (evicted_key, _) = cache
                    .pending
                    .pop_first()
                    .expect("a full pending cache must contain an entry");
                cache.remove_key_if_idle(&evicted_key);
            }
            cache.pending.insert(
                request.key.clone(),
                PendingServerResponseCacheWrite {
                    revision: request.revision,
                    response,
                },
            );
            if cache.worker_running {
                false
            } else {
                cache.worker_running = true;
                true
            }
        };
        if start_worker {
            let state = self.clone();
            drop(tokio::spawn(async move {
                state.run_server_response_cache_writer().await;
            }));
        }
    }

    async fn run_server_response_cache_writer(&self) {
        loop {
            let Some((key, pending)) = ({
                let mut cache = self
                    .inner
                    .server_response_cache
                    .lock()
                    .expect("server response cache mutex poisoned");
                let pending = cache.pending.pop_first();
                if let Some((key, pending)) = &pending {
                    cache.active = Some((key.clone(), pending.revision));
                } else {
                    cache.worker_running = false;
                }
                pending
            }) else {
                return;
            };

            let _write = self.inner.server_response_cache_write.lock().await;
            let is_latest = {
                let cache = self
                    .inner
                    .server_response_cache
                    .lock()
                    .expect("server response cache mutex poisoned");
                pending.revision >= cache.minimum_revision
                    && cache
                        .keys
                        .get(&key)
                        .and_then(|state| state.latest_success_revision)
                        == Some(pending.revision)
            };
            if is_latest
                && let Err(error) =
                    save_cached_server_response(&self.inner.pool, &key.0, &key.1, &pending.response)
                        .await
            {
                tracing::warn!("failed to cache Server response: {error}");
            }
            drop(_write);

            let mut cache = self
                .inner
                .server_response_cache
                .lock()
                .expect("server response cache mutex poisoned");
            if cache
                .active
                .as_ref()
                .is_some_and(|(active, revision)| active == &key && *revision == pending.revision)
            {
                cache.active = None;
            }
            cache.remove_key_if_idle(&key);
        }
    }

    async fn begin_server_response_cache_transition(
        &self,
    ) -> Result<ServerResponseCacheTransition<'_>, DaemonError> {
        let write = self.inner.server_response_cache_write.lock().await;
        {
            let mut cache = self
                .inner
                .server_response_cache
                .lock()
                .expect("server response cache mutex poisoned");
            cache.invalidate();
        }
        clear_server_response_cache(&self.inner.pool).await?;
        Ok(ServerResponseCacheTransition {
            inner: self.inner.as_ref(),
            _write: write,
            published: false,
        })
    }

    pub(crate) fn project_config_view(&self) -> DaemonProjectConfig {
        let project_config = self.project_config();
        let readiness = project_config.readiness();
        DaemonProjectConfig {
            server_url: project_config.server_url,
            project_id: project_config.project_id,
            memory_guidelines_path: project_config.memory_guidelines_path,
            has_access_token: project_config.access_token.is_some(),
            has_refresh_token: project_config.refresh_token.is_some(),
            ready: readiness.ready,
            missing_fields: readiness.missing_fields,
        }
    }

    pub async fn health(&self) -> DaemonHealth {
        let schema_version = current_schema_version(&self.inner.pool).await.unwrap_or(0);
        let project_config = self.project_config();
        DaemonHealth {
            daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
            agent_runtime: AgentRuntimeIdentity {
                protocol_revision: agent_runtime::AGENT_RUNTIME_PROTOCOL_REVISION,
                build_id: agent_runtime::AGENT_RUNTIME_BUILD_ID.to_owned(),
            },
            server_url: project_config.server_url,
            project_id: project_config.project_id,
            daemon_installation_id: self.inner.daemon_installation_id.clone(),
            log_dir: self.inner.config.logs_dir().display().to_string(),
            local_db: LocalDbStatus {
                path: self.inner.config.local_db_path().display().to_string(),
                ready: schema_version == CURRENT_LOCAL_SCHEMA_VERSION,
                schema_version,
            },
        }
    }

    pub async fn sync_status(&self) -> Result<DaemonSyncStatus, DaemonError> {
        load_sync_status(self).await
    }

    pub async fn project_sync_status(
        &self,
        request: DaemonProjectSyncStatusRequest,
    ) -> Result<DaemonSyncStatus, DaemonError> {
        load_project_sync_status(self, &request.project_id).await
    }

    pub async fn project_storage(
        &self,
        request: DaemonProjectStorageRequest,
    ) -> Result<DaemonProjectStorage, DaemonError> {
        project_storage::project_storage(self, request).await
    }

    pub async fn replace_project_storage(
        &self,
        request: DaemonProjectStorageReplaceRequest,
    ) -> Result<DaemonProjectStorageMove, DaemonError> {
        project_storage::replace_project_storage(self, request).await
    }

    pub async fn project_storage_move(
        &self,
        request: DaemonProjectStorageMoveRequest,
    ) -> Result<DaemonProjectStorageMove, DaemonError> {
        project_storage::project_storage_move(self, request).await
    }

    pub async fn reset_project_storage(
        &self,
        request: DaemonProjectStorageResetRequest,
    ) -> Result<DaemonProjectStorageMove, DaemonError> {
        project_storage::reset_project_storage(self, request).await
    }

    pub async fn clear_project_cache(
        &self,
        request: DaemonProjectCacheClearRequest,
    ) -> Result<DaemonProjectStorage, DaemonError> {
        project_storage::clear_project_cache(self, request).await
    }

    pub async fn memory_cache(
        &self,
        request: DaemonMemoryCacheRequest,
    ) -> Result<DaemonMemoryCacheStatus, DaemonError> {
        commit_sync::memory_cache(self, request).await
    }

    pub async fn project_checkout(
        &self,
        request: DaemonProjectCheckoutRequest,
    ) -> Result<DaemonProjectCheckout, DaemonError> {
        commit_sync::project_checkout(self, request).await
    }

    pub async fn activate_memory(
        &self,
        request: ActivateMemoryRequest,
    ) -> Result<ActivateMemoryResponse, DaemonError> {
        search::activate_memory(self, request).await
    }

    pub async fn load_memory(
        &self,
        request: LoadMemoryRequest,
    ) -> Result<LoadMemoryResponse, DaemonError> {
        search::load_memory(self, request).await
    }

    pub async fn search_index_status(
        &self,
        request: SearchIndexProjectRequest,
    ) -> Result<SearchIndexStatus, DaemonError> {
        search::search_index_status(self, request).await
    }

    pub async fn rebuild_search_index(
        &self,
        request: SearchIndexProjectRequest,
    ) -> Result<SearchIndexStatus, DaemonError> {
        search::rebuild_search_index(self, request).await
    }

    pub async fn list_retrieval_runs(
        &self,
        request: RetrievalRunListRequest,
    ) -> Result<RetrievalRunListResponse, DaemonError> {
        retrieval_history::list_retrieval_runs(self, request).await
    }

    pub async fn get_retrieval_run(
        &self,
        request: RetrievalRunRequest,
    ) -> Result<RetrievalRunDetail, DaemonError> {
        retrieval_history::get_retrieval_run(self, request).await
    }

    pub async fn list_recalls(
        &self,
        request: ListRecallsRequest,
    ) -> Result<ListRecallsResponse, DaemonError> {
        recall::list_recalls(self, request).await
    }

    pub async fn get_recall_fragment(
        &self,
        request: GetRecallFragmentRequest,
    ) -> Result<GetRecallFragmentResponse, DaemonError> {
        recall::get_recall_fragment(self, request).await
    }

    pub async fn create_evaluation_case(
        &self,
        request: CreateEvaluationCaseRequest,
    ) -> Result<EvaluationCaseDetail, DaemonError> {
        retrieval_history::create_evaluation_case(self, request).await
    }

    pub async fn resolve_evaluation_case(
        &self,
        request: ResolveEvaluationCaseRequest,
    ) -> Result<EvaluationCaseDetail, DaemonError> {
        retrieval_history::resolve_evaluation_case(self, request).await
    }

    pub async fn clear_retrieval_runs(
        &self,
        request: ClearRetrievalRunsRequest,
    ) -> Result<ClearRetrievalRunsResponse, DaemonError> {
        retrieval_history::clear_retrieval_runs(self, request).await
    }

    pub async fn export_evaluation_set(
        &self,
        request: ExportEvaluationSetRequest,
    ) -> Result<ExportEvaluationSetResponse, DaemonError> {
        retrieval_history::export_evaluation_set(self, request).await
    }

    pub async fn retry_sync(
        &self,
        request: DaemonSyncRetryRequest,
    ) -> Result<DaemonRetryResponse, DaemonError> {
        self.retry_sync_scoped(request.channel, None).await
    }

    pub async fn project_retry_sync(
        &self,
        request: DaemonProjectSyncRetryRequest,
    ) -> Result<DaemonRetryResponse, DaemonError> {
        commit_sync::validate_cache_component("project_id", &request.project_id)?;
        self.retry_sync_scoped(request.channel, Some(&request.project_id))
            .await
    }

    async fn retry_sync_scoped(
        &self,
        channel: SyncRetryChannel,
        project_id: Option<&str>,
    ) -> Result<DaemonRetryResponse, DaemonError> {
        let retry_id = format!("retry_{}", Uuid::new_v4().simple());

        sqlx::query(
            "INSERT INTO sync_retries (retry_id, channel)
             VALUES ($1, $2)",
        )
        .bind(&retry_id)
        .bind(channel.as_str())
        .execute(&self.inner.pool)
        .await?;

        let retry_drafts = matches!(channel, SyncRetryChannel::Drafts | SyncRetryChannel::All);
        let retry_commits = matches!(channel, SyncRetryChannel::Commits | SyncRetryChannel::All);
        if retry_drafts {
            sqlx::query(
                "UPDATE local_draft_operations
                 SET sync_status = 'queued', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE sync_status IN ('retrying', 'failed')
                   AND (
                       $1 IS NULL OR draft_id IN (
                           SELECT draft_id FROM local_drafts WHERE project_id = $1
                       )
                   )",
            )
            .bind(project_id)
            .execute(&self.inner.pool)
            .await?;
        }
        self.run_sync_channels(retry_drafts, retry_commits, false, project_id)
            .await?;

        Ok(DaemonRetryResponse {
            retry_id,
            started: true,
        })
    }

    pub async fn store_draft_operation(
        &self,
        request: DaemonDraftOperationRequest,
    ) -> Result<DaemonDraftOperationResponse, DaemonError> {
        request.op.validate(request.resource)?;
        let has_text_update = request.op.has_text_update();
        let is_mcp_target_mutation = request.source == Some(DaemonDraftOperationSource::McpStore)
            && request.op.discard.is_none()
            && (request.op.update.is_some()
                || request.op.rename.is_some()
                || request.op.delete.is_some());
        if has_text_update || is_mcp_target_mutation {
            let _sync_guard = self.inner.sync_lock.lock().await;
            let _mutation_guard = self.inner.draft_mutation_lock.lock().await;
            let request = if has_text_update {
                self.materialize_text_update(request).await?
            } else {
                self.normalize_mcp_target_mutation(request).await?
            };
            return self.persist_draft_operation(request).await;
        }
        if request.op.delete.is_some() || request.op.discard.is_some() {
            let _sync_guard = self.inner.sync_lock.lock().await;
            let _mutation_guard = self.inner.draft_mutation_lock.lock().await;
            let request = if request.source == Some(DaemonDraftOperationSource::McpStore)
                && request.op.discard.is_some()
            {
                self.normalize_mcp_discard(request).await?
            } else {
                request
            };
            return self.persist_draft_operation(request).await;
        }

        let _mutation_guard = self.inner.draft_mutation_lock.lock().await;
        self.persist_draft_operation(request).await
    }

    async fn normalize_mcp_target_mutation(
        &self,
        mut request: DaemonDraftOperationRequest,
    ) -> Result<DaemonDraftOperationRequest, DaemonError> {
        let target_id = request.op.target_id().ok_or_else(|| {
            DaemonError::InvalidRequest(
                "target-backed MCP mutation requires a stable Memory resource id".to_owned(),
            )
        })?;
        // A create Draft is itself the effective resource until publication.
        // Resolve that local identity before consulting the materialized
        // Project index: delete must become Discard even when no Project Ref
        // has been activated yet, and rename must keep the Draft's real scope.
        if let Some(local) = sqlx::query(
            "SELECT draft_id, resource_scope
             FROM local_drafts
             WHERE draft_id = $1
               AND project_id = $2
               AND resource_kind = $3
               AND target_id IS NULL
               AND status IN ('open', 'submitted')",
        )
        .bind(target_id)
        .bind(&request.project_id)
        .bind(request.resource.as_str())
        .fetch_optional(&self.inner.pool)
        .await?
        {
            request.draft_id = Some(local.try_get("draft_id")?);
            request.scope = daemon_draft_scope_from_str(
                local.try_get::<String, _>("resource_scope")?.as_str(),
            )?;
            return Ok(request);
        }
        let resource = self
            .load_stable_mutation_target(&request, target_id)
            .await?;
        request.scope = match resource.scope {
            SourceScope::Org => DaemonDraftScope::Org,
            SourceScope::Project => {
                return Err(legacy_project_memory_read_only_error(&resource.resource_id));
            }
        };
        Ok(request)
    }

    async fn load_stable_mutation_target(
        &self,
        request: &DaemonDraftOperationRequest,
        target_id: &str,
    ) -> Result<LoadedMemoryResource, DaemonError> {
        let loaded = search::load_memory(
            self,
            LoadMemoryRequest {
                project_id: request.project_id.clone(),
                ids: vec![target_id.to_owned()],
                known_hashes: BTreeMap::new(),
            },
        )
        .await?;
        let resource = loaded.resources.into_iter().next().ok_or_else(|| {
            DaemonError::NotFound(format!("memory resource {target_id} is not available"))
        })?;
        if resource.resource_id != target_id {
            return Err(DaemonError::InvalidRequest(
                "draft mutation id must be a stable resource id, not a path".to_owned(),
            ));
        }
        if !memory_kind_matches_resource(resource.kind, request.resource) {
            return Err(DaemonError::InvalidRequest(
                "draft mutation resource kind does not match its target".to_owned(),
            ));
        }
        Ok(resource)
    }

    async fn materialize_text_update(
        &self,
        mut request: DaemonDraftOperationRequest,
    ) -> Result<DaemonDraftOperationRequest, DaemonError> {
        let update = request
            .op
            .update
            .take()
            .and_then(DaemonUpdateDraftOperation::into_text)
            .ok_or_else(|| {
                DaemonError::InvalidRequest(
                    "draft operation does not contain a text replacement update".to_owned(),
                )
            })?;
        let resource = self
            .load_stable_mutation_target(&request, &update.id)
            .await?;
        match resource.scope {
            SourceScope::Org => request.scope = DaemonDraftScope::Org,
            SourceScope::Project
                if request.source == Some(DaemonDraftOperationSource::McpStore) =>
            {
                return Err(legacy_project_memory_read_only_error(&resource.resource_id));
            }
            SourceScope::Project => request.scope = DaemonDraftScope::Project,
        }
        if resource.content_hash != update.expected_hash {
            return Err(DaemonError::State {
                code: "memory_content_changed",
                message: format!(
                    "Memory resource {} changed from expected hash {}; reload it before updating (current hash: {})",
                    update.id, update.expected_hash, resource.content_hash
                ),
            });
        }
        let content = resource.content.ok_or_else(|| DaemonError::State {
            code: "memory_content_unavailable",
            message: format!(
                "Memory resource {} did not include content for replacement",
                update.id
            ),
        })?;
        let content = apply_exact_text_replacements(&content, &update.replacements)?;
        request.op.update = Some(DaemonUpdateDraftOperation::Content(
            DaemonContentDraftUpdate {
                id: update.id,
                content: DaemonDraftContent::from_resource(request.resource, content),
                description: update.description,
            },
        ));
        request.op.validate(request.resource)?;
        Ok(request)
    }

    async fn normalize_mcp_discard(
        &self,
        mut request: DaemonDraftOperationRequest,
    ) -> Result<DaemonDraftOperationRequest, DaemonError> {
        let draft_id = request
            .op
            .discard
            .as_ref()
            .map(|discard| discard.id.as_str())
            .ok_or_else(|| {
                DaemonError::InvalidRequest(
                    "MCP discard requires an existing local Draft id".to_owned(),
                )
            })?;
        let row = sqlx::query(
            "SELECT draft_id, resource_scope
             FROM local_drafts
             WHERE draft_id = $1
               AND project_id = $2
               AND resource_kind = $3",
        )
        .bind(draft_id)
        .bind(&request.project_id)
        .bind(request.resource.as_str())
        .fetch_optional(&self.inner.pool)
        .await?
        .ok_or_else(|| DaemonError::NotFound(format!("local draft not found: {draft_id}")))?;
        request.draft_id = Some(row.try_get("draft_id")?);
        request.scope =
            daemon_draft_scope_from_str(row.try_get::<String, _>("resource_scope")?.as_str())?;
        Ok(request)
    }

    async fn persist_draft_operation(
        &self,
        mut request: DaemonDraftOperationRequest,
    ) -> Result<DaemonDraftOperationResponse, DaemonError> {
        if request.scope == DaemonDraftScope::Project
            && (request.op.discard.is_none() || request.draft_id.is_none())
        {
            return Err(project_memory_authority_removed_error());
        }
        let source = request
            .source
            .unwrap_or(DaemonDraftOperationSource::Desktop);
        let requested_base_commit_id = request.base_commit_id;
        let new_draft_base_commit_id = match requested_base_commit_id.as_deref() {
            Some(commit_id) => Some(commit_id.to_owned()),
            None => {
                commit_sync::current_base_commit_id(
                    &self.inner.pool,
                    &request.project_id,
                    request.scope,
                )
                .await?
            }
        };
        // This transaction reads the current draft before writing both the
        // operation and its index invalidation. Reserve SQLite's writer slot
        // up front: a concurrent index-worker commit between that read and
        // the first write would otherwise fail with BUSY_SNAPSHOT (517),
        // which is not retried by busy_timeout.
        let mut tx = self.inner.pool.begin_with("BEGIN IMMEDIATE").await?;

        let draft_id = resolve_local_draft(
            &mut tx,
            LocalDraftResolutionInput {
                requested_draft_id: request.draft_id.as_deref(),
                project_id: &request.project_id,
                requested_base_commit_id: requested_base_commit_id.as_deref(),
                new_draft_base_commit_id: new_draft_base_commit_id.as_deref(),
                scope: request.scope,
                resource: request.resource,
                op: &mut request.op,
            },
        )
        .await?;
        let operation_json = serde_json::to_string(&request.op)?;
        let local_operation_id = format!("op_{}", Uuid::new_v4().simple());

        sqlx::query(
            "INSERT INTO local_draft_operations (
                local_operation_id, draft_id, resource_kind, operation_json, source, sync_status
             )
             VALUES ($1, $2, $3, $4, $5, 'queued')",
        )
        .bind(&local_operation_id)
        .bind(&draft_id)
        .bind(request.resource.as_str())
        .bind(operation_json)
        .bind(source.as_str())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE local_drafts
             SET reconciliation = 'unknown', reconciliation_candidate_id = NULL,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE draft_id = $1",
        )
        .bind(&draft_id)
        .execute(&mut *tx)
        .await?;

        search::scheduler::enqueue_project_in_tx(&mut tx, &request.project_id).await?;
        tx.commit().await?;
        self.inner.search_index_notify.notify_one();
        self.request_sync();

        Ok(DaemonDraftOperationResponse {
            local_operation_id,
            draft_id,
            queued: true,
            sync_status: DraftOperationSyncStatus::Queued,
        })
    }

    pub async fn list_drafts(
        &self,
        query: DaemonDraftListQuery,
    ) -> Result<DaemonDraftListResponse, DaemonError> {
        list_local_drafts(&self.inner.pool, query).await
    }

    pub async fn get_draft(&self, draft_id: &str) -> Result<DaemonDraftDetail, DaemonError> {
        load_local_draft_detail(&self.inner.pool, draft_id).await
    }

    async fn drain_draft_queue(&self, project_id: Option<&str>) -> Result<bool, DaemonError> {
        drain_draft_queue(self, project_id).await
    }

    async fn pull_draft_events(&self) -> Result<(), DaemonError> {
        pull_draft_events(self).await
    }

    pub fn start_sync_worker(&self) -> Option<JoinHandle<()>> {
        if !self.inner.config.sync.enabled {
            return None;
        }

        let state = self.clone();
        Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(state.inner.config.sync.interval);
            let mut failures = (0, String::new());
            loop {
                let retry_transient_failures = tokio::select! {
                    _ = interval.tick() => true,
                    _ = state.inner.sync_notify.notified() => false,
                };
                crate::diagnostics::REQUEST_ID
                    .scope(crate::diagnostics::new_request_id(), async {
                        let result = state.run_sync_cycle(retry_transient_failures).await;
                        crate::diagnostics::worker_result("sync", &mut failures, &result);
                    })
                    .await;
            }
        }))
    }

    pub fn start_search_model_worker(&self) -> JoinHandle<()> {
        let state = self.clone();
        let models = self.inner.search_models.clone();
        models.begin_preparation();
        tokio::spawn(async move {
            let mut retry_delay = Duration::from_secs(5);
            loop {
                let attempt_models = models.clone();
                let result = tokio::task::spawn_blocking(move || attempt_models.prepare()).await;
                match result {
                    Ok(Ok(())) => {
                        if let Err(error) =
                            search::scheduler::enqueue_all_cached_projects(&state).await
                        {
                            tracing::warn!(
                                "failed to schedule search indexes after model preparation: {error}"
                            );
                        }
                        break;
                    }
                    Ok(Err(error)) => {
                        tracing::warn!("search model preparation failed: {}", error.message);
                    }
                    Err(error) => {
                        tracing::error!("search model preparation worker failed: {error}");
                    }
                }
                tokio::time::sleep(retry_delay).await;
                retry_delay = retry_delay.saturating_mul(2).min(Duration::from_secs(60));
                models.begin_preparation();
            }
        })
    }

    pub fn start_search_index_worker(&self) -> JoinHandle<()> {
        search::scheduler::start_worker(self)
    }

    pub fn request_sync(&self) {
        if self.inner.config.sync.enabled {
            self.inner.sync_notify.notify_one();
        }
    }

    async fn run_sync_cycle(&self, retry_transient_failures: bool) -> Result<(), DaemonError> {
        self.run_sync_channels(true, true, retry_transient_failures, None)
            .await
    }

    async fn run_sync_channels(
        &self,
        sync_drafts: bool,
        sync_commits: bool,
        retry_transient_failures: bool,
        project_id: Option<&str>,
    ) -> Result<(), DaemonError> {
        let _sync_guard = self.inner.sync_lock.lock().await;
        async {
            if !self.project_config().server_readiness().ready {
                return Ok(());
            }
            let mut first_error = None;
            if sync_drafts {
                let last_attempt_key =
                    crate::draft::draft_sync_meta_key(META_DRAFT_SYNC_LAST_ATTEMPT_AT, project_id);
                let last_success_key =
                    crate::draft::draft_sync_meta_key(META_DRAFT_SYNC_LAST_SUCCESS_AT, project_id);
                let draft_result = async {
                    if retry_transient_failures {
                        queue_retrying_operations(&self.inner.pool).await?;
                    }
                    upsert_meta_timestamp(&self.inner.pool, &last_attempt_key).await?;
                    let queue_converged = self.drain_draft_queue(project_id).await?;
                    self.pull_draft_events().await?;
                    if queue_converged {
                        upsert_meta_timestamp(&self.inner.pool, &last_success_key).await?;
                    }
                    Ok::<(), DaemonError>(())
                }
                .await;
                if let Err(error) = draft_result {
                    first_error = Some(error);
                }
            }
            if sync_commits {
                let commit_result = match project_id {
                    Some(project_id) => commit_sync::run_for_project(self, project_id).await,
                    None => commit_sync::run(self).await,
                };
                if let Err(error) = commit_result
                    && first_error.is_none()
                {
                    first_error = Some(error);
                }
            }
            match first_error {
                Some(error) => Err(error),
                None => Ok(()),
            }
        }
        .await
    }
}

async fn load_startup_credentials(
    credential_store: Arc<dyn CredentialStore>,
    timeout: Duration,
) -> Option<ServerCredentials> {
    match tokio::time::timeout(timeout, load_server_credentials(credential_store)).await {
        Ok(Ok(credentials)) => credentials,
        Ok(Err(error)) => {
            tracing::warn!(
                "Server credentials are unavailable; continuing daemon startup without authentication: {error}"
            );
            None
        }
        Err(_) => {
            tracing::warn!(
                "Timed out loading Server credentials after {} ms; continuing daemon startup without authentication",
                timeout.as_millis()
            );
            None
        }
    }
}

macro_rules! dispatch_async {
    ($self:expr, $payload:expr, $method:ident) => {{
        let payload = $self.decode_dispatch_payload::<_>($payload);
        match payload {
            Ok(payload) => $self
                .$method(payload)
                .await
                .and_then(|value| serde_json::to_value(value).map_err(DaemonError::from)),
            Err(error) => Err(error),
        }
    }};
}

macro_rules! dispatch_result_async {
    ($self:expr, $method:ident) => {
        $self
            .$method()
            .await
            .and_then(|value| serde_json::to_value(value).map_err(DaemonError::from))
    };
}

macro_rules! dispatch_value {
    ($self:expr, $method:ident) => {
        serde_json::to_value($self.$method()).map_err(DaemonError::from)
    };
    ($self:expr, $method:ident, async) => {
        serde_json::to_value($self.$method().await).map_err(DaemonError::from)
    };
}

#[derive(Clone)]
pub struct DaemonIpcService {
    state: DaemonState,
}

impl std::ops::Deref for DaemonIpcService {
    type Target = DaemonState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DaemonIpcService {
    pub fn new(state: DaemonState) -> Self {
        Self { state }
    }

    pub fn project_config(&self) -> DaemonProjectConfig {
        self.state.project_config_status()
    }

    pub async fn dispatch(&self, request: DaemonIpcRequest) -> DaemonIpcResponse {
        let id = crate::diagnostics::validated_request_id(&request.request_id)
            .unwrap_or_else(crate::diagnostics::new_request_id);
        crate::diagnostics::REQUEST_ID.scope(id.clone(), async {
            let started = std::time::Instant::now();
            // IPC method names are identifiers, not caller-supplied paths or payloads.
            let method = crate::diagnostics::validated_request_id(&request.method).unwrap_or_else(|| "invalid".to_owned());
            let response = self.dispatch_request(request).await;
            if let Some(error) = &response.error {
                tracing::warn!(event = "ipc_failed", request_id = %id, method, elapsed_ms = started.elapsed().as_millis() as u64, code = %error.code, details = %error.details);
            } else if method == "server_request" {
                tracing::info!(event = "ipc_completed", request_id = %id, method, elapsed_ms = started.elapsed().as_millis() as u64);
            } else {
                tracing::debug!(event = "ipc_completed", request_id = %id, method, elapsed_ms = started.elapsed().as_millis() as u64);
            }
            response
        }).await
    }

    async fn dispatch_request(&self, request: DaemonIpcRequest) -> DaemonIpcResponse {
        let result = match request.method.as_str() {
            "health" => dispatch_value!(self, health, async),
            "project_config" => dispatch_value!(self, project_config),
            "replace_project_config" => {
                dispatch_async!(self, request.payload, replace_project_config)
            }
            "select_project" => dispatch_async!(self, request.payload, select_project),
            "resolve_project_binding" => {
                dispatch_async!(self, request.payload, resolve_project_binding)
            }
            "list_project_bindings" => {
                dispatch_async!(self, request.payload, list_project_bindings)
            }
            "replace_project_binding" => {
                dispatch_async!(self, request.payload, replace_project_binding)
            }
            "remove_project_binding" => {
                dispatch_async!(self, request.payload, remove_project_binding)
            }
            "list_project_agent_adapters" => {
                dispatch_async!(self, request.payload, list_project_agent_adapters)
            }
            "list_all_project_agent_adapters" => {
                dispatch_result_async!(self, list_all_project_agent_adapters)
            }
            "inspect_legacy_agent_adapters" => {
                dispatch_async!(self, request.payload, inspect_legacy_agent_adapters)
            }
            "agent_adapter_settings" => {
                dispatch_result_async!(self, agent_adapter_settings)
            }
            "set_agent_adapter" => {
                dispatch_async!(self, request.payload, set_agent_adapter)
            }
            "inspect_codex_plugin" => {
                dispatch_async!(self, request.payload, inspect_codex_plugin)
            }
            "reconcile_codex_plugin" => {
                dispatch_async!(self, request.payload, reconcile_codex_plugin)
            }
            "install_project_agent_adapter" => {
                dispatch_async!(self, request.payload, install_project_agent_adapter)
            }
            "remove_project_agent_adapter" => {
                dispatch_async!(self, request.payload, remove_project_agent_adapter)
            }
            "sync_status" => dispatch_result_async!(self, sync_status),
            "project_sync_status" => {
                dispatch_async!(self, request.payload, project_sync_status)
            }
            "project_storage" => dispatch_async!(self, request.payload, project_storage),
            "replace_project_storage" => {
                dispatch_async!(self, request.payload, replace_project_storage)
            }
            "project_storage_move" => {
                dispatch_async!(self, request.payload, project_storage_move)
            }
            "reset_project_storage" => {
                dispatch_async!(self, request.payload, reset_project_storage)
            }
            "clear_project_cache" => {
                dispatch_async!(self, request.payload, clear_project_cache)
            }
            "memory_cache" => dispatch_async!(self, request.payload, memory_cache),
            "project_checkout" => dispatch_async!(self, request.payload, project_checkout),
            "activate_memory" => dispatch_async!(self, request.payload, activate_memory),
            "load_memory" => dispatch_async!(self, request.payload, load_memory),
            "search_index_status" => {
                dispatch_async!(self, request.payload, search_index_status)
            }
            "rebuild_search_index" => {
                dispatch_async!(self, request.payload, rebuild_search_index)
            }
            "list_retrieval_runs" => {
                dispatch_async!(self, request.payload, list_retrieval_runs)
            }
            "get_retrieval_run" => dispatch_async!(self, request.payload, get_retrieval_run),
            "list_recalls" => dispatch_async!(self, request.payload, list_recalls),
            "get_recall_fragment" => {
                dispatch_async!(self, request.payload, get_recall_fragment)
            }
            "create_evaluation_case" => {
                dispatch_async!(self, request.payload, create_evaluation_case)
            }
            "resolve_evaluation_case" => {
                dispatch_async!(self, request.payload, resolve_evaluation_case)
            }
            "clear_retrieval_runs" => {
                dispatch_async!(self, request.payload, clear_retrieval_runs)
            }
            "export_evaluation_set" => {
                dispatch_async!(self, request.payload, export_evaluation_set)
            }
            "retry_sync" => dispatch_async!(self, request.payload, retry_sync),
            "project_retry_sync" => {
                dispatch_async!(self, request.payload, project_retry_sync)
            }
            "list_drafts" => dispatch_async!(self, request.payload, list_drafts),
            "get_draft" => {
                let payload =
                    self.decode_dispatch_payload::<DaemonDraftDetailRequest>(request.payload);
                match payload {
                    Ok(payload) => self
                        .get_draft(&payload.draft_id)
                        .await
                        .and_then(|value| serde_json::to_value(value).map_err(DaemonError::from)),
                    Err(error) => Err(error),
                }
            }
            "store_draft_operation" | "desktop_store_draft_operation" => {
                dispatch_async!(self, request.payload, store_draft_operation)
            }
            "server_request" => dispatch_async!(self, request.payload, server_request),
            method => Err(DaemonError::InvalidRequest(format!(
                "unknown daemon IPC method: {method}"
            ))),
        };
        DaemonIpcResponse::from_result(result)
    }

    fn decode_dispatch_payload<T>(&self, payload: serde_json::Value) -> Result<T, DaemonError>
    where
        T: DeserializeOwned,
    {
        serde_json::from_value(payload).map_err(DaemonError::from)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Barrier, Condvar, Mutex as StdMutex};

    use super::*;
    use crate::search::SearchFailure;
    use crate::search::models::{SearchModelRuntimeStatus, SearchModels};

    #[tokio::test]
    async fn http_client_times_out_hanging_response() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
            drop(stream);
        });

        let client = build_http_client(Duration::from_secs(1), Duration::from_millis(50))
            .expect("build test HTTP client");
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            client.get(format!("http://{address}")).send(),
        )
        .await
        .expect("HTTP request timeout must remain bounded")
        .expect_err("a hanging response must time out");

        assert!(error.is_timeout(), "expected timeout error, got {error}");
        server.abort();
    }

    #[tokio::test]
    async fn http_client_negotiates_and_decodes_gzip() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        const BODY: &str = "compressed response";
        const GZIP_BODY: &[u8] = &[
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x4b, 0xce, 0xcf, 0x2d,
            0x28, 0x4a, 0x2d, 0x2e, 0x4e, 0x4d, 0x51, 0x00, 0x52, 0x05, 0xf9, 0x79, 0xc5, 0xa9,
            0x00, 0xb1, 0xff, 0x32, 0x6f, 0x13, 0x00, 0x00, 0x00,
        ];

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut buffer = [0_u8; 1024];
                let read = stream.read(&mut buffer).await.unwrap();
                assert_ne!(read, 0, "client closed before completing HTTP headers");
                request.extend_from_slice(&buffer[..read]);
                assert!(
                    request.len() <= 8 * 1024,
                    "request headers are unexpectedly large"
                );
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
            let accepts_gzip = request.lines().any(|line| {
                line.split_once(':').is_some_and(|(name, value)| {
                    name.trim() == "accept-encoding"
                        && value.split(',').any(|encoding| encoding.trim() == "gzip")
                })
            });
            assert!(accepts_gzip, "client did not negotiate gzip: {request}");

            let response_head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-encoding: gzip\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                GZIP_BODY.len()
            );
            stream.write_all(response_head.as_bytes()).await.unwrap();
            stream.write_all(GZIP_BODY).await.unwrap();
            stream.shutdown().await.unwrap();
        });

        let client = build_http_client(Duration::from_secs(1), Duration::from_secs(1))
            .expect("build test HTTP client");
        let response = client
            .get(format!("http://{address}"))
            .send()
            .await
            .expect("send compressed request");
        assert_eq!(response.text().await.unwrap(), BODY);
        server.await.unwrap();
    }

    struct BlockingCredentialStore {
        gate: Arc<(StdMutex<bool>, Condvar)>,
    }

    impl CredentialStore for BlockingCredentialStore {
        fn load(&self) -> Result<Option<ServerCredentials>, CredentialStoreError> {
            let (ready, wake) = &*self.gate;
            let ready = ready.lock().unwrap();
            let _ = wake
                .wait_timeout_while(ready, Duration::from_secs(1), |ready| !*ready)
                .unwrap();
            Ok(Some(ServerCredentials {
                server_url: "https://clumsies.example.test".to_owned(),
                access_token: "access-token".to_owned(),
                refresh_token: None,
            }))
        }

        fn replace(&self, _credentials: &ServerCredentials) -> Result<(), CredentialStoreError> {
            Ok(())
        }

        fn clear(&self) -> Result<(), CredentialStoreError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn startup_continues_when_credential_store_blocks() {
        let gate = Arc::new((StdMutex::new(false), Condvar::new()));
        let store = Arc::new(BlockingCredentialStore { gate: gate.clone() });

        let credentials = tokio::time::timeout(
            Duration::from_millis(250),
            load_startup_credentials(store, Duration::from_millis(10)),
        )
        .await
        .expect("startup credential fallback must remain bounded");

        assert!(credentials.is_none());
        let (ready, wake) = &*gate;
        *ready.lock().unwrap() = true;
        wake.notify_all();
    }

    #[test]
    fn startup_credential_load_timeout_tolerates_slow_keychain_reads() {
        // A Keychain read right after unlock was measured at ~3.7 s; the
        // timeout must stay well above that so a slow-but-valid read does
        // not silently drop the session at startup.
        assert!(
            STARTUP_CREDENTIAL_LOAD_TIMEOUT >= Duration::from_secs(5),
            "startup credential load timeout must tolerate slow Keychain reads"
        );
    }

    struct DeferredCredentialStore {
        credentials: Arc<StdMutex<Option<ServerCredentials>>>,
        load_count: Arc<AtomicUsize>,
        replace_gate: Option<(Arc<Barrier>, Arc<Barrier>)>,
        load_gate: StdMutex<Option<(Arc<Barrier>, Arc<Barrier>)>>,
        fail_replace: AtomicBool,
    }

    impl DeferredCredentialStore {
        fn new() -> Self {
            Self {
                credentials: Arc::new(StdMutex::new(None)),
                load_count: Arc::new(AtomicUsize::new(0)),
                replace_gate: None,
                load_gate: StdMutex::new(None),
                fail_replace: AtomicBool::new(false),
            }
        }

        fn with_replace_gate(entered: Arc<Barrier>, release: Arc<Barrier>) -> Self {
            Self {
                replace_gate: Some((entered, release)),
                ..Self::new()
            }
        }

        fn gate_next_load(&self, entered: Arc<Barrier>, release: Arc<Barrier>) {
            *self.load_gate.lock().unwrap() = Some((entered, release));
        }

        fn set(&self, credentials: ServerCredentials) {
            *self.credentials.lock().unwrap() = Some(credentials);
        }

        fn credentials(&self) -> Option<ServerCredentials> {
            self.credentials.lock().unwrap().clone()
        }

        fn fail_replace(&self) {
            self.fail_replace.store(true, Ordering::SeqCst);
        }

        fn load_count(&self) -> usize {
            self.load_count.load(Ordering::SeqCst)
        }
    }

    impl CredentialStore for DeferredCredentialStore {
        fn load(&self) -> Result<Option<ServerCredentials>, CredentialStoreError> {
            self.load_count.fetch_add(1, Ordering::SeqCst);
            let credentials = self.credentials.lock().unwrap().clone();
            let gate = self.load_gate.lock().unwrap().take();
            if let Some((entered, release)) = gate {
                entered.wait();
                release.wait();
            }
            Ok(credentials)
        }

        fn replace(&self, credentials: &ServerCredentials) -> Result<(), CredentialStoreError> {
            if let Some((entered, release)) = &self.replace_gate {
                entered.wait();
                release.wait();
            }
            if self.fail_replace.load(Ordering::SeqCst) {
                return Err(CredentialStoreError::new(
                    "injected credential replace failure",
                ));
            }
            *self.credentials.lock().unwrap() = Some(credentials.clone());
            Ok(())
        }

        fn clear(&self) -> Result<(), CredentialStoreError> {
            *self.credentials.lock().unwrap() = None;
            Ok(())
        }
    }

    struct StubSearchModels;

    impl SearchModels for StubSearchModels {
        fn revision(&self) -> Result<String, SearchFailure> {
            Ok("stub-models.v1".to_owned())
        }

        fn token_offsets(&self, text: &str) -> Result<Vec<(usize, usize)>, SearchFailure> {
            Ok(text
                .char_indices()
                .map(|(start, character)| (start, start + character.len_utf8()))
                .collect())
        }

        fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, SearchFailure> {
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

    async fn test_state(store: Arc<DeferredCredentialStore>) -> (tempfile::TempDir, DaemonState) {
        let root = tempfile::tempdir().unwrap();
        let mut config = DaemonConfig::for_root(root.path().join("daemon"));
        config.project.server_url = "https://clumsies.example.test".to_owned();
        config.project.project_id = Some("prj_test".to_owned());
        let state = DaemonState::initialize_with_credential_store_and_search_models(
            config,
            store,
            Arc::new(StubSearchModels),
        )
        .await
        .unwrap();
        (root, state)
    }

    async fn unauthenticated_test_state(
        store: Arc<DeferredCredentialStore>,
    ) -> (tempfile::TempDir, DaemonState) {
        let (root, state) = test_state(store).await;
        assert!(state.project_config().access_token.is_none());
        (root, state)
    }

    #[tokio::test]
    async fn global_adapter_choices_apply_to_every_bound_project_and_keep_binding_required() {
        let (root, state) =
            unauthenticated_test_state(Arc::new(DeferredCredentialStore::new())).await;
        let choices = state.agent_adapter_settings().await.unwrap();
        assert_eq!(
            choices
                .items
                .iter()
                .filter(|item| item.enabled)
                .map(|item| item.adapter)
                .collect::<Vec<_>>(),
            vec![ProjectAgentAdapterKind::Codex]
        );
        assert!(choices.items.iter().all(|item| !item.configured));
        sqlx::query("INSERT INTO host_agent_adapters VALUES ('claude-code', 1, 1, NULL)")
            .execute(&state.inner.pool)
            .await
            .unwrap();
        let required = ProjectAgentAdapterRuntimeRequirement {
            adapter: ProjectAgentAdapterKind::ClaudeCode,
            delivery: ProjectAgentAdapterDelivery::LegacyFiles,
        };
        for project in ["one", "two"] {
            let workspace = root.path().join(project);
            std::fs::create_dir_all(&workspace).unwrap();
            let workspace = std::fs::canonicalize(workspace)
                .unwrap()
                .display()
                .to_string();
            sqlx::query("INSERT INTO project_bindings (server_url, workspace_root, project_id, revision) VALUES ($1, $2, $3, 1)")
                .bind(canonical_server_url(&state.project_config().server_url).unwrap()).bind(&workspace).bind(project)
                .execute(&state.inner.pool).await.unwrap();
            let binding = state
                .resolve_project_binding(DaemonProjectBindingResolveRequest {
                    workspace_path: workspace,
                    required_adapter: Some(required),
                })
                .await
                .unwrap();
            assert_eq!(binding.project_id, project);
        }
        assert!(
            state
                .resolve_project_binding(DaemonProjectBindingResolveRequest {
                    workspace_path: root.path().display().to_string(),
                    required_adapter: Some(required),
                })
                .await
                .is_err()
        );
        sqlx::query("UPDATE host_agent_adapters SET enabled = 0 WHERE adapter = 'claude-code'")
            .execute(&state.inner.pool)
            .await
            .unwrap();
        assert!(
            state
                .resolve_project_binding(DaemonProjectBindingResolveRequest {
                    workspace_path: root.path().join("one").display().to_string(),
                    required_adapter: Some(required),
                })
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn lazy_recovery_restores_session_when_store_becomes_available() {
        let store = Arc::new(DeferredCredentialStore::new());
        let (_root, state) = unauthenticated_test_state(store.clone()).await;

        store.set(ServerCredentials {
            server_url: "https://clumsies.example.test".to_owned(),
            access_token: "lazy-access".to_owned(),
            refresh_token: Some("lazy-refresh".to_owned()),
        });

        let config = state.project_config_with_credentials().await;
        assert_eq!(config.access_token.as_deref(), Some("lazy-access"));
        assert_eq!(config.refresh_token.as_deref(), Some("lazy-refresh"));
        assert_eq!(
            state.project_config().access_token.as_deref(),
            Some("lazy-access")
        );
    }

    #[tokio::test]
    async fn lazy_recovery_ignores_credentials_for_another_server() {
        let store = Arc::new(DeferredCredentialStore::new());
        let (_root, state) = unauthenticated_test_state(store.clone()).await;

        store.set(ServerCredentials {
            server_url: "https://other.example.test".to_owned(),
            access_token: "foreign-access".to_owned(),
            refresh_token: None,
        });

        let config = state.project_config_with_credentials().await;
        assert!(config.access_token.is_none());
        assert!(state.project_config().access_token.is_none());
    }

    #[tokio::test]
    async fn lazy_recovery_skips_credential_store_when_session_is_present() {
        let store = Arc::new(DeferredCredentialStore::new());
        let (_root, state) = unauthenticated_test_state(store.clone()).await;
        state
            .replace_project_config(DaemonProjectConfigUpdateRequest {
                server_url: "https://clumsies.example.test".to_owned(),
                project_id: Some("prj_test".to_owned()),
                memory_guidelines_path: None,
                access_token: Some("existing-access".to_owned()),
                refresh_token: Some("existing-refresh".to_owned()),
            })
            .await
            .unwrap();
        let loads_before = store.load_count();

        let config = state.project_config_with_credentials().await;

        assert_eq!(config.access_token.as_deref(), Some("existing-access"));
        assert_eq!(store.load_count(), loads_before);
    }

    #[tokio::test]
    async fn lazy_recovery_is_rate_limited_after_failed_attempt() {
        let store = Arc::new(DeferredCredentialStore::new());
        let (_root, state) = unauthenticated_test_state(store.clone()).await;

        let first = state.project_config_with_credentials().await;
        let second = state.project_config_with_credentials().await;

        assert!(first.access_token.is_none());
        assert!(second.access_token.is_none());
        assert_eq!(store.load_count(), 2);
    }
    #[tokio::test]
    async fn lazy_recovery_clears_cached_responses_before_session_activation() {
        let store = Arc::new(DeferredCredentialStore::new());
        let (_root, state) = unauthenticated_test_state(store.clone()).await;
        let path = "/api/v1/lazy-session-cache";
        let server_url = state.project_config().server_url;
        seed_server_response_cache(&state, path).await;
        let before = state.project_config_snapshot();

        store.set(ServerCredentials {
            server_url: server_url.clone(),
            access_token: "lazy-access".to_owned(),
            refresh_token: Some("lazy-refresh".to_owned()),
        });

        let recovered = state.project_config_with_credentials_snapshot().await;

        assert_eq!(
            recovered.config.access_token.as_deref(),
            Some("lazy-access")
        );
        assert!(recovered.session_revision > before.session_revision);
        assert!(
            load_cached_server_response(&state.inner.pool, &server_url, path)
                .await
                .unwrap()
                .is_none(),
            "activating a recovered session must not expose another session's cached response"
        );
    }

    #[tokio::test]
    async fn lazy_recovery_cache_delete_failure_keeps_session_inactive() {
        let store = Arc::new(DeferredCredentialStore::new());
        let (_root, state) = unauthenticated_test_state(store.clone()).await;
        let path = "/api/v1/lazy-session-cache-delete-failure";
        let server_url = state.project_config().server_url;
        seed_server_response_cache(&state, path).await;
        reject_server_response_cache_deletes(&state).await;
        let before = state.project_config_snapshot();

        store.set(ServerCredentials {
            server_url: server_url.clone(),
            access_token: "lazy-access".to_owned(),
            refresh_token: Some("lazy-refresh".to_owned()),
        });

        let recovered = state.project_config_with_credentials_snapshot().await;

        assert!(recovered.config.access_token.is_none());
        assert_eq!(recovered.session_revision, before.session_revision);
        assert!(state.project_config().access_token.is_none());
        assert!(
            load_cached_server_response(&state.inner.pool, &server_url, path)
                .await
                .unwrap()
                .is_some(),
            "a failed cache delete must leave the recovered identity unpublished"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lazy_recovery_does_not_overwrite_a_concurrent_config_replace() {
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let store = Arc::new(DeferredCredentialStore::new());
        let (_root, state) = unauthenticated_test_state(store.clone()).await;
        store.set(ServerCredentials {
            server_url: "https://clumsies.example.test".to_owned(),
            access_token: "stale-access".to_owned(),
            refresh_token: Some("stale-refresh".to_owned()),
        });
        store.gate_next_load(entered.clone(), release.clone());

        let recovery_state = state.clone();
        let recovery = tokio::spawn(async move {
            recovery_state
                .project_config_with_credentials_snapshot()
                .await
        });
        tokio::task::spawn_blocking(move || entered.wait())
            .await
            .unwrap();

        state
            .replace_project_config(DaemonProjectConfigUpdateRequest {
                server_url: "https://replacement.example.test".to_owned(),
                project_id: Some("prj_replacement".to_owned()),
                memory_guidelines_path: None,
                access_token: Some("replacement-access".to_owned()),
                refresh_token: Some("replacement-refresh".to_owned()),
            })
            .await
            .unwrap();
        tokio::task::spawn_blocking(move || release.wait())
            .await
            .unwrap();

        let recovered = recovery.await.unwrap();
        assert_eq!(
            recovered.config.server_url,
            "https://replacement.example.test"
        );
        assert_eq!(
            recovered.config.access_token.as_deref(),
            Some("replacement-access")
        );
        let persisted = store.credentials().unwrap();
        assert_eq!(persisted.server_url, "https://replacement.example.test");
        assert_eq!(persisted.access_token, "replacement-access");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lazy_recovery_preserves_a_concurrent_project_selection() {
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let store = Arc::new(DeferredCredentialStore::new());
        let (_root, state) = unauthenticated_test_state(store.clone()).await;
        store.set(ServerCredentials {
            server_url: "https://clumsies.example.test".to_owned(),
            access_token: "lazy-access".to_owned(),
            refresh_token: Some("lazy-refresh".to_owned()),
        });
        store.gate_next_load(entered.clone(), release.clone());

        let recovery_state = state.clone();
        let recovery = tokio::spawn(async move {
            recovery_state
                .project_config_with_credentials_snapshot()
                .await
        });
        tokio::task::spawn_blocking(move || entered.wait())
            .await
            .unwrap();
        state
            .select_project(DaemonProjectSelectionRequest {
                project_id: "prj_selected".to_owned(),
            })
            .await
            .unwrap();
        tokio::task::spawn_blocking(move || release.wait())
            .await
            .unwrap();

        let recovered = recovery.await.unwrap();
        assert_eq!(recovered.config.project_id.as_deref(), Some("prj_selected"));
        assert_eq!(
            recovered.config.access_token.as_deref(),
            Some("lazy-access")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn project_selection_waits_for_credential_transition() {
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let store = Arc::new(DeferredCredentialStore::with_replace_gate(
            entered.clone(),
            release.clone(),
        ));
        let (_root, state) = unauthenticated_test_state(store.clone()).await;

        let replace_state = state.clone();
        let replace = tokio::spawn(async move {
            replace_state
                .replace_project_config(DaemonProjectConfigUpdateRequest {
                    server_url: "https://replacement.example.test".to_owned(),
                    project_id: Some("prj_replacement".to_owned()),
                    memory_guidelines_path: None,
                    access_token: Some("replacement-access".to_owned()),
                    refresh_token: Some("replacement-refresh".to_owned()),
                })
                .await
        });
        tokio::task::spawn_blocking(move || entered.wait())
            .await
            .unwrap();

        let select_state = state.clone();
        let mut select = tokio::spawn(async move {
            select_state
                .select_project(DaemonProjectSelectionRequest {
                    project_id: "prj_selected".to_owned(),
                })
                .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut select)
                .await
                .is_err(),
            "project selection must wait for the in-flight credential transition"
        );

        tokio::task::spawn_blocking(move || release.wait())
            .await
            .unwrap();
        replace.await.unwrap().unwrap();
        select.await.unwrap().unwrap();

        let config = state.project_config();
        assert_eq!(config.server_url, "https://replacement.example.test");
        assert_eq!(config.project_id.as_deref(), Some("prj_selected"));
        assert_eq!(config.access_token.as_deref(), Some("replacement-access"));
        let persisted = store.credentials().unwrap();
        assert_eq!(persisted.server_url, "https://replacement.example.test");
        assert_eq!(persisted.access_token, "replacement-access");
    }

    async fn wait_for_server_response_cache_worker(state: &DaemonState) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let running = state
                    .inner
                    .server_response_cache
                    .lock()
                    .expect("server response cache mutex poisoned")
                    .worker_running;
                if !running {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("bounded Server response cache writer should become idle");
    }

    async fn wait_for_active_server_response_cache_write(state: &DaemonState) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let active = state
                    .inner
                    .server_response_cache
                    .lock()
                    .expect("server response cache mutex poisoned")
                    .active
                    .is_some();
                if active {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("Server response cache writer should claim its queued write");
    }

    async fn reject_server_response_cache_deletes(state: &DaemonState) {
        sqlx::query(
            "CREATE TRIGGER reject_server_response_cache_delete
             BEFORE DELETE ON server_response_cache
             BEGIN
                 SELECT RAISE(ABORT, 'injected Server response cache delete failure');
             END",
        )
        .execute(&state.inner.pool)
        .await
        .unwrap();
    }

    async fn seed_server_response_cache(state: &DaemonState, path: &str) {
        save_cached_server_response(
            &state.inner.pool,
            &state.project_config().server_url,
            path,
            &DaemonServerResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: r#"{"account":"old"}"#.to_owned(),
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn queued_cache_writes_keep_the_newest_response() {
        let store = Arc::new(DeferredCredentialStore::new());
        let (_root, state) = unauthenticated_test_state(store).await;
        let path = "/api/v1/cache-order".to_owned();
        let write_gate = state.inner.server_response_cache_write.lock().await;
        let older = state.start_server_response_cache_request(&path);
        let newer = state.start_server_response_cache_request(&path);
        let server_url = newer.server_url().to_owned();
        state.queue_server_response_cache_write(
            &newer,
            &DaemonServerResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: r#"{"version":"newer"}"#.to_owned(),
            },
        );
        state.queue_server_response_cache_write(
            &older,
            &DaemonServerResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: r#"{"version":"older"}"#.to_owned(),
            },
        );
        drop(older);
        drop(newer);
        drop(write_gate);

        wait_for_server_response_cache_worker(&state).await;
        let cached = load_cached_server_response(&state.inner.pool, &server_url, &path)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cached.body, r#"{"version":"newer"}"#);
    }

    #[tokio::test]
    async fn cache_writer_bounds_distinct_paths_and_body_copies() {
        let store = Arc::new(DeferredCredentialStore::new());
        let (_root, state) = unauthenticated_test_state(store).await;
        let write_gate = state.inner.server_response_cache_write.lock().await;

        for index in 0..SERVER_RESPONSE_CACHE_MAX_PENDING_WRITES * 4 {
            let request =
                state.start_server_response_cache_request(&format!("/api/v1/cache-{index}"));
            state.queue_server_response_cache_write(
                &request,
                &DaemonServerResponse {
                    status: 200,
                    headers: BTreeMap::new(),
                    body: format!("response-{index}"),
                },
            );
        }
        let oversized_path = "/api/v1/cache-oversized";
        let oversized = state.start_server_response_cache_request(oversized_path);
        state.queue_server_response_cache_write(
            &oversized,
            &DaemonServerResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: "x".repeat(SERVER_RESPONSE_CACHE_MAX_BODY_BYTES + 1),
            },
        );
        drop(oversized);
        tokio::task::yield_now().await;

        {
            let cache = state
                .inner
                .server_response_cache
                .lock()
                .expect("server response cache mutex poisoned");
            let active_count = usize::from(cache.active.is_some());
            assert!(cache.worker_running);
            assert!(cache.pending.len() <= SERVER_RESPONSE_CACHE_MAX_PENDING_WRITES);
            assert!(
                cache.pending.len() + active_count <= SERVER_RESPONSE_CACHE_MAX_PENDING_WRITES + 1
            );
            assert!(
                cache.keys.len() <= SERVER_RESPONSE_CACHE_MAX_PENDING_WRITES + 1,
                "finished request watermarks must not grow with distinct paths"
            );
            assert!(
                !cache
                    .pending
                    .contains_key(&(state.project_config().server_url, oversized_path.to_owned())),
                "oversized responses must not be copied into the pending cache"
            );
        }

        drop(write_gate);
        wait_for_server_response_cache_worker(&state).await;
        let cache = state
            .inner
            .server_response_cache
            .lock()
            .expect("server response cache mutex poisoned");
        assert!(cache.pending.is_empty());
        assert!(cache.active.is_none());
        assert!(cache.keys.is_empty());
    }

    #[tokio::test]
    async fn cache_delete_failure_precedes_project_identity_mutations() {
        let store = Arc::new(DeferredCredentialStore::new());
        let (_root, state) = unauthenticated_test_state(store.clone()).await;
        let original = state.project_config();
        seed_server_response_cache(&state, "/api/v1/cache-delete-failure").await;
        reject_server_response_cache_deletes(&state).await;

        let error = state
            .replace_project_config(DaemonProjectConfigUpdateRequest {
                server_url: "https://replacement.example.test".to_owned(),
                project_id: Some("prj_replacement".to_owned()),
                memory_guidelines_path: None,
                access_token: Some("replacement-access".to_owned()),
                refresh_token: None,
            })
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("injected Server response cache delete failure")
        );
        let runtime = state.project_config();
        assert_eq!(runtime.server_url, original.server_url);
        assert_eq!(runtime.project_id, original.project_id);
        assert!(runtime.access_token.is_none());
        assert!(store.credentials().is_none());
        assert!(
            load_meta_value(&state.inner.pool, "project_config_server_url")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            load_meta_value(&state.inner.pool, "project_config_project_id")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn cache_delete_failure_does_not_clear_tokens() {
        let store = Arc::new(DeferredCredentialStore::new());
        store.set(ServerCredentials {
            server_url: "https://clumsies.example.test".to_owned(),
            access_token: "existing-access".to_owned(),
            refresh_token: Some("existing-refresh".to_owned()),
        });
        let (_root, state) = test_state(store.clone()).await;
        seed_server_response_cache(&state, "/api/v1/cache-clear-token-failure").await;
        reject_server_response_cache_deletes(&state).await;

        let snapshot = state.project_config_snapshot();
        let error = state
            .clear_server_tokens_if_current(
                snapshot.session_revision,
                &snapshot.config.server_url,
                snapshot.config.access_token.as_deref().unwrap(),
            )
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("injected Server response cache delete failure")
        );
        let runtime = state.project_config();
        assert_eq!(runtime.access_token.as_deref(), Some("existing-access"));
        assert_eq!(runtime.refresh_token.as_deref(), Some("existing-refresh"));
        let persisted = store
            .credentials()
            .expect("cache failure must leave Keychain credentials untouched");
        assert_eq!(persisted.access_token, "existing-access");
        assert_eq!(persisted.refresh_token.as_deref(), Some("existing-refresh"));
    }

    #[tokio::test]
    async fn post_commit_retry_queue_failure_does_not_fail_config_replace() {
        let store = Arc::new(DeferredCredentialStore::new());
        let (_root, state) = unauthenticated_test_state(store.clone()).await;
        sqlx::query("DROP TABLE local_draft_operations")
            .execute(&state.inner.pool)
            .await
            .unwrap();

        let replaced = state
            .replace_project_config(DaemonProjectConfigUpdateRequest {
                server_url: "https://replacement.example.test".to_owned(),
                project_id: Some("prj_replacement".to_owned()),
                memory_guidelines_path: None,
                access_token: Some("replacement-access".to_owned()),
                refresh_token: Some("replacement-refresh".to_owned()),
            })
            .await
            .expect("post-commit retry queue failure must not report a rolled-back config");

        assert!(replaced.has_access_token);
        let runtime = state.project_config();
        assert_eq!(runtime.server_url, "https://replacement.example.test");
        assert_eq!(runtime.access_token.as_deref(), Some("replacement-access"));
        let persisted = store.credentials().unwrap();
        assert_eq!(persisted.server_url, "https://replacement.example.test");
        assert_eq!(persisted.access_token, "replacement-access");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_credential_transition_discards_queued_cache_writes() {
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let store = Arc::new(DeferredCredentialStore::with_replace_gate(
            entered.clone(),
            release.clone(),
        ));
        store.fail_replace();
        let (_root, state) = unauthenticated_test_state(store.clone()).await;
        let path = "/api/v1/cache-failed-credential-transition".to_owned();

        let replace_state = state.clone();
        let replace = tokio::spawn(async move {
            replace_state
                .replace_project_config(DaemonProjectConfigUpdateRequest {
                    server_url: "https://replacement.example.test".to_owned(),
                    project_id: Some("prj_replacement".to_owned()),
                    memory_guidelines_path: None,
                    access_token: Some("replacement-access".to_owned()),
                    refresh_token: None,
                })
                .await
        });
        tokio::task::spawn_blocking(move || entered.wait())
            .await
            .unwrap();

        let request = state.start_server_response_cache_request(&path);
        let server_url = request.server_url().to_owned();
        state.queue_server_response_cache_write(
            &request,
            &DaemonServerResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: r#"{"account":"old"}"#.to_owned(),
            },
        );
        drop(request);
        wait_for_active_server_response_cache_write(&state).await;

        tokio::task::spawn_blocking(move || release.wait())
            .await
            .unwrap();
        let error = replace.await.unwrap().unwrap_err();
        assert!(
            error
                .to_string()
                .contains("injected credential replace failure")
        );
        wait_for_server_response_cache_worker(&state).await;

        assert!(
            load_cached_server_response(&state.inner.pool, &server_url, &path)
                .await
                .unwrap()
                .is_none(),
            "an unpublished transition must discard queued old-account responses"
        );
        assert!(store.credentials().is_none());
        assert!(state.project_config().access_token.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn config_change_blocks_cache_writes_during_credential_transition() {
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let store = Arc::new(DeferredCredentialStore::with_replace_gate(
            entered.clone(),
            release.clone(),
        ));
        let (_root, state) = unauthenticated_test_state(store.clone()).await;
        let path = "/api/v1/cache-session-transition".to_owned();

        let replace_state = state.clone();
        let replace = tokio::spawn(async move {
            replace_state
                .replace_project_config(DaemonProjectConfigUpdateRequest {
                    server_url: "https://clumsies.example.test".to_owned(),
                    project_id: Some("prj_test".to_owned()),
                    memory_guidelines_path: None,
                    access_token: Some("replacement-access".to_owned()),
                    refresh_token: None,
                })
                .await
        });
        tokio::task::spawn_blocking(move || entered.wait())
            .await
            .unwrap();

        let transition = state.start_server_response_cache_request(&path);
        let server_url = transition.server_url().to_owned();
        state.queue_server_response_cache_write(
            &transition,
            &DaemonServerResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: r#"{"account":"old"}"#.to_owned(),
            },
        );
        drop(transition);
        wait_for_active_server_response_cache_write(&state).await;
        reject_server_response_cache_deletes(&state).await;
        assert!(
            load_cached_server_response(&state.inner.pool, &server_url, &path)
                .await
                .unwrap()
                .is_none(),
            "the transition barrier must prevent the old account response from reaching SQLite"
        );

        tokio::task::spawn_blocking(move || release.wait())
            .await
            .unwrap();
        replace.await.unwrap().unwrap();
        wait_for_server_response_cache_worker(&state).await;
        assert!(
            load_cached_server_response(&state.inner.pool, &server_url, &path)
                .await
                .unwrap()
                .is_none(),
            "the old account response must not survive the runtime session change"
        );
        let credentials = store
            .credentials()
            .expect("successful transition must persist replacement credentials");
        assert_eq!(credentials.access_token, "replacement-access");
        assert_eq!(
            state.project_config().access_token.as_deref(),
            Some("replacement-access")
        );
    }
}
