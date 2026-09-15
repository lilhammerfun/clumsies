use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Connection, Row, SqlitePool};
use uuid::Uuid;

use crate::config::{CURRENT_LOCAL_SCHEMA_VERSION, META_MEMORY_CACHE_RESET_REQUIRED};
use crate::util::non_empty_string;
use crate::{
    CredentialStore, CredentialStoreError, DaemonConfig, DaemonError, ProjectConfig,
    RuntimeProjectConfig, ServerCredentials,
};
use crate::{
    agent_adapter, commit_sync, project_storage, retrieval_history, search, work_tracking,
};

pub(crate) fn prepare_directories(config: &DaemonConfig) -> Result<(), DaemonError> {
    project_storage::ensure_private_directory(&config.root_dir)?;
    project_storage::ensure_private_directory(&config.cache_dir)?;
    project_storage::ensure_private_directory(&config.logs_dir())?;
    Ok(())
}

pub(crate) async fn connect_local_db(path: &Path) -> Result<SqlitePool, DaemonError> {
    let options = SqliteConnectOptions::from_str(&path.display().to_string())?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5))
        .synchronous(SqliteSynchronous::Normal);
    Ok(SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?)
}

pub(crate) async fn migrate_local_db(pool: &SqlitePool) -> Result<(), DaemonError> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS daemon_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    let mut existing_schema_version = current_schema_version(pool).await?;
    if existing_schema_version == 13 {
        migrate_local_schema_13_to_14(pool).await?;
        existing_schema_version = 14;
    }
    if existing_schema_version == 14 {
        migrate_local_schema_14_to_15(pool).await?;
        existing_schema_version = 15;
    }
    if existing_schema_version == 15 {
        migrate_local_schema_15_to_16(pool).await?;
        existing_schema_version = 16;
    }
    if existing_schema_version == 16 {
        migrate_local_schema_16_to_17(pool).await?;
        existing_schema_version = 17;
    }
    if existing_schema_version == 17 {
        migrate_local_schema_17_to_18(pool).await?;
        existing_schema_version = 18;
    }
    if existing_schema_version == 18 {
        migrate_local_schema_18_to_19(pool).await?;
        existing_schema_version = 19;
    }
    if existing_schema_version == 19 {
        migrate_local_schema_19_to_20(pool).await?;
        existing_schema_version = 20;
    }
    if existing_schema_version == 20 {
        migrate_local_schema_20_to_21(pool).await?;
        existing_schema_version = 21;
    }
    if existing_schema_version == 21 {
        migrate_local_schema_21_to_22(pool).await?;
        existing_schema_version = 22;
    }
    if existing_schema_version == 22 {
        migrate_local_schema_22_to_23(pool).await?;
        existing_schema_version = 23;
    }
    if existing_schema_version == 23 {
        migrate_local_schema_23_to_24(pool).await?;
        existing_schema_version = 24;
    }
    if existing_schema_version == 24 {
        migrate_local_schema_24_to_25(pool).await?;
        existing_schema_version = 25;
    }
    if existing_schema_version == 25 {
        migrate_local_schema_25_to_26(pool).await?;
        existing_schema_version = 26;
    }
    if existing_schema_version == 26 {
        migrate_local_schema_26_to_27(pool).await?;
        existing_schema_version = 27;
    }
    if existing_schema_version == 27 {
        migrate_local_schema_27_to_28(pool).await?;
        existing_schema_version = 28;
    }
    if existing_schema_version == 28 {
        migrate_local_schema_28_to_29(pool).await?;
        existing_schema_version = 29;
    }
    if existing_schema_version == 29 {
        migrate_local_schema_29_to_30(pool).await?;
        existing_schema_version = 30;
    }
    if existing_schema_version == 30 {
        migrate_local_schema_30_to_31(pool).await?;
        existing_schema_version = 31;
    }
    if existing_schema_version == 31 {
        migrate_local_schema_31_to_32(pool).await?;
        existing_schema_version = 32;
    }
    if existing_schema_version == 32 {
        migrate_local_schema_32_to_33(pool).await?;
        existing_schema_version = 33;
    }
    if existing_schema_version == 33 {
        migrate_local_schema_33_to_34(pool).await?;
        existing_schema_version = 34;
    }
    if existing_schema_version == 34 {
        migrate_local_schema_34_to_35(pool).await?;
        existing_schema_version = 35;
    }
    if existing_schema_version == 35 {
        migrate_local_schema_35_to_36(pool).await?;
        existing_schema_version = 36;
    }
    if existing_schema_version == 36 {
        migrate_local_schema_36_to_37(pool).await?;
        existing_schema_version = 37;
    }
    if existing_schema_version == 37 {
        migrate_local_schema_37_to_38(pool).await?;
        existing_schema_version = 38;
    }
    if existing_schema_version == 38 {
        migrate_local_schema_38_to_39(pool).await?;
        existing_schema_version = 39;
    }
    if existing_schema_version == 39 {
        migrate_local_schema_39_to_40(pool).await?;
        existing_schema_version = 40;
    }
    if existing_schema_version == 40 {
        agent_adapter::global::migrate(pool).await?;
        sqlx::query("UPDATE daemon_meta SET value = '41' WHERE key = 'schema_version'")
            .execute(pool)
            .await?;
        existing_schema_version = 41;
    }
    if existing_schema_version != 0 && existing_schema_version != CURRENT_LOCAL_SCHEMA_VERSION {
        return Err(DaemonError::InvalidConfig(format!(
            "local database schema version {existing_schema_version} is incompatible with version {CURRENT_LOCAL_SCHEMA_VERSION}; recreate the daemon database"
        )));
    }
    sqlx::query(
        "DELETE FROM daemon_meta
         WHERE key IN ('project_config_access_token', 'project_config_refresh_token')",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS local_drafts (
            draft_id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            server_draft_id TEXT,
            server_version BIGINT NOT NULL DEFAULT 0,
            base_commit_id TEXT,
            current_commit_id TEXT,
            freshness TEXT NOT NULL CHECK (freshness IN ('current', 'behind')) DEFAULT 'current',
            has_upstream_resource_changes INTEGER NOT NULL CHECK (has_upstream_resource_changes IN (0, 1)) DEFAULT 0,
            reconciliation TEXT NOT NULL CHECK (reconciliation IN ('unknown', 'clean', 'conflicts')) DEFAULT 'unknown',
            reconciliation_candidate_id TEXT,
            resource_scope TEXT NOT NULL CHECK (resource_scope IN ('org', 'project')),
            resource_kind TEXT NOT NULL CHECK (resource_kind IN ('context', 'rule', 'workflow', 'memory')),
            target_id TEXT,
            path TEXT,
            status TEXT NOT NULL CHECK (status IN ('open', 'submitted', 'merged', 'discarded')) DEFAULT 'open',
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_local_drafts_target_id
         ON local_drafts (target_id)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_local_drafts_server_draft_id
         ON local_drafts (server_draft_id)
         WHERE server_draft_id IS NOT NULL",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS local_draft_operations (
            local_operation_id TEXT PRIMARY KEY,
            draft_id TEXT NOT NULL REFERENCES local_drafts(draft_id) ON DELETE CASCADE,
            server_operation_id TEXT,
            resource_kind TEXT NOT NULL CHECK (resource_kind IN ('context', 'rule', 'workflow', 'memory')),
            operation_json TEXT NOT NULL,
            source TEXT NOT NULL CHECK (source IN ('desktop', 'cli', 'mcp_store', 'server')),
            sync_status TEXT NOT NULL CHECK (sync_status IN ('queued', 'syncing', 'retrying', 'synced', 'failed')),
            last_error TEXT,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS server_response_cache (
            server_url TEXT NOT NULL,
            path TEXT NOT NULL,
            status BIGINT NOT NULL,
            headers_json TEXT NOT NULL,
            body TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            PRIMARY KEY (server_url, path)
        )",
    )
    .execute(pool)
    .await?;
    create_project_bindings_table(pool).await?;
    agent_adapter::migrate(pool).await?;
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_local_draft_operations_server_operation_id
         ON local_draft_operations (server_operation_id)
         WHERE server_operation_id IS NOT NULL",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_local_draft_operations_sync_status
         ON local_draft_operations (sync_status)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS sync_retries (
            retry_id TEXT PRIMARY KEY,
            channel TEXT NOT NULL CHECK (channel IN ('drafts', 'commits', 'all')),
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS remote_draft_events (
            event_id TEXT PRIMARY KEY,
            draft_id TEXT NOT NULL,
            project_id TEXT NOT NULL,
            event_type TEXT NOT NULL,
            version BIGINT NOT NULL,
            daemon_installation_id TEXT,
            created_at TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    commit_sync::migrate(pool).await?;
    project_storage::migrate(pool).await?;
    search::migrate(pool).await?;
    retrieval_history::migrate(pool).await?;
    work_tracking::migrate(pool).await?;
    sqlx::query(
        "INSERT INTO daemon_meta (key, value)
         VALUES ('schema_version', $1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(CURRENT_LOCAL_SCHEMA_VERSION.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn migrate_local_schema_20_to_21(pool: &SqlitePool) -> Result<(), DaemonError> {
    let local_drafts_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM sqlite_master
            WHERE type = 'table' AND name = 'local_drafts'
         )",
    )
    .fetch_one(pool)
    .await?;
    if !local_drafts_exists {
        return Ok(());
    }
    sqlx::query(
        "ALTER TABLE local_drafts
         ADD COLUMN has_upstream_resource_changes INTEGER NOT NULL
         CHECK (has_upstream_resource_changes IN (0, 1)) DEFAULT 0",
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn migrate_local_schema_21_to_22(pool: &SqlitePool) -> Result<(), DaemonError> {
    work_tracking::migrate(pool).await
}

pub(crate) async fn migrate_local_schema_22_to_23(pool: &SqlitePool) -> Result<(), DaemonError> {
    work_tracking::migrate(pool).await
}

pub(crate) async fn migrate_local_schema_23_to_24(pool: &SqlitePool) -> Result<(), DaemonError> {
    work_tracking::migrate(pool).await
}

pub(crate) async fn migrate_local_schema_24_to_25(pool: &SqlitePool) -> Result<(), DaemonError> {
    work_tracking::migrate(pool).await
}

pub(crate) async fn migrate_local_schema_25_to_26(pool: &SqlitePool) -> Result<(), DaemonError> {
    work_tracking::migrate(pool).await
}

pub(crate) async fn migrate_local_schema_27_to_28(pool: &SqlitePool) -> Result<(), DaemonError> {
    let mut tx = pool.begin().await?;
    let table_sql: Option<String> = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'agent_runs'",
    )
    .fetch_optional(&mut *tx)
    .await?;
    if table_sql
        .as_deref()
        .is_some_and(|sql| sql.contains("'manual'"))
    {
        tx.commit().await?;
        return Ok(());
    }
    if table_sql.is_none() {
        // A library that never ran the schema-25 migration has no agent_runs
        // table at all; create the v28 shape directly instead of rebuilding.
        for statement in [
            "CREATE TABLE agent_runs (
                run_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                issue_number BIGINT CHECK (issue_number IS NULL OR issue_number > 0),
                host TEXT NOT NULL CHECK (host IN ('codex', 'claude-code', 'zed', 'manual')),
                host_run_key TEXT NOT NULL,
                host_session_id TEXT,
                parent_run_id TEXT REFERENCES agent_runs(run_id),
                kind TEXT NOT NULL CHECK (kind IN ('root', 'subagent')),
                phase TEXT NOT NULL CHECK (phase IN ('running', 'ended')),
                outcome TEXT CHECK (outcome IN ('completed', 'blocked', 'failed', 'cancelled', 'unknown')),
                end_reason TEXT,
                display_label TEXT,
                summary TEXT,
                revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
                start_observed INTEGER NOT NULL DEFAULT 1 CHECK (start_observed IN (0, 1)),
                started_at TEXT NOT NULL,
                last_seen_at TEXT NOT NULL,
                lease_expires_at TEXT NOT NULL,
                ended_at TEXT,
                UNIQUE (project_id, host, host_run_key)
            )",
            "CREATE INDEX idx_agent_runs_project_issue_latest
             ON agent_runs (project_id, issue_number, last_seen_at DESC, run_id DESC)",
            "CREATE INDEX idx_agent_runs_running_lease
             ON agent_runs (phase, lease_expires_at)",
            "CREATE INDEX idx_agent_runs_project_session
             ON agent_runs (project_id, host, host_session_id, phase)",
        ] {
            sqlx::query(statement).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        return Ok(());
    }
    for statement in [
        "CREATE TABLE agent_runs_v28 (
            run_id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            issue_number BIGINT CHECK (issue_number IS NULL OR issue_number > 0),
            host TEXT NOT NULL CHECK (host IN ('codex', 'claude-code', 'zed', 'manual')),
            host_run_key TEXT NOT NULL,
            host_session_id TEXT,
            parent_run_id TEXT REFERENCES agent_runs(run_id),
            kind TEXT NOT NULL CHECK (kind IN ('root', 'subagent')),
            phase TEXT NOT NULL CHECK (phase IN ('running', 'ended')),
            outcome TEXT CHECK (outcome IN ('completed', 'blocked', 'failed', 'cancelled', 'unknown')),
            end_reason TEXT,
            display_label TEXT,
            summary TEXT,
            revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
            start_observed INTEGER NOT NULL DEFAULT 1 CHECK (start_observed IN (0, 1)),
            started_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL,
            lease_expires_at TEXT NOT NULL,
            ended_at TEXT,
            UNIQUE (project_id, host, host_run_key)
        )",
        "INSERT INTO agent_runs_v28 (
            run_id, project_id, issue_number, host, host_run_key, host_session_id,
            parent_run_id, kind, phase, outcome, end_reason, display_label, summary,
            revision, start_observed, started_at, last_seen_at, lease_expires_at, ended_at
         )
         SELECT run_id, project_id, issue_number, host, host_run_key, host_session_id,
                parent_run_id, kind, phase, outcome, end_reason, display_label, summary,
                revision, start_observed, started_at, last_seen_at, lease_expires_at, ended_at
         FROM agent_runs",
        "DROP TABLE agent_runs",
        "ALTER TABLE agent_runs_v28 RENAME TO agent_runs",
        "CREATE INDEX idx_agent_runs_project_issue_latest
         ON agent_runs (project_id, issue_number, last_seen_at DESC, run_id DESC)",
        "CREATE INDEX idx_agent_runs_running_lease
         ON agent_runs (phase, lease_expires_at)",
        "CREATE INDEX idx_agent_runs_project_session
         ON agent_runs (project_id, host, host_session_id, phase)",
    ] {
        sqlx::query(statement).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn migrate_local_schema_26_to_27(pool: &SqlitePool) -> Result<(), DaemonError> {
    work_tracking::migrate(pool).await
}

pub(crate) async fn migrate_local_schema_28_to_29(pool: &SqlitePool) -> Result<(), DaemonError> {
    work_tracking::migrate(pool).await
}

/// Widen the agent_runs host CHECK constraint to accept the opencode plugin
/// integration host. SQLite cannot alter a CHECK constraint in place, so the
/// table is rebuilt following the same pattern as schema 27 to 28.
///
/// The rebuild drops agent_runs, which issue_workflow_states references via
/// changed_by_run_id. sqlx enables foreign key enforcement by default, so the
/// drop would fail; foreign keys are disabled for this connection during the
/// rebuild only. A single dedicated connection is used because the PRAGMA is
/// per-connection.
pub(crate) async fn migrate_local_schema_29_to_30(pool: &SqlitePool) -> Result<(), DaemonError> {
    let mut connection = pool.acquire().await?;
    let table_sql: Option<String> = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'agent_runs'",
    )
    .fetch_optional(&mut *connection)
    .await?;
    if table_sql
        .as_deref()
        .is_some_and(|sql| sql.contains("'opencode'"))
    {
        return Ok(());
    }
    if table_sql.is_none() {
        // A library that never ran schema 28 has no agent_runs table at all;
        // the work_tracking::migrate path already creates the widened shape.
        return Ok(());
    }
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *connection)
        .await?;
    let mut tx = connection.begin().await?;
    for statement in [
        "CREATE TABLE agent_runs_v30 (
            run_id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            issue_number BIGINT CHECK (issue_number IS NULL OR issue_number > 0),
            host TEXT NOT NULL CHECK (host IN ('codex', 'claude-code', 'zed', 'manual', 'opencode')),
            host_run_key TEXT NOT NULL,
            host_session_id TEXT,
            parent_run_id TEXT REFERENCES agent_runs(run_id),
            kind TEXT NOT NULL CHECK (kind IN ('root', 'subagent')),
            phase TEXT NOT NULL CHECK (phase IN ('running', 'ended')),
            outcome TEXT CHECK (outcome IN ('completed', 'blocked', 'failed', 'cancelled', 'unknown')),
            end_reason TEXT,
            display_label TEXT,
            summary TEXT,
            revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
            start_observed INTEGER NOT NULL DEFAULT 1 CHECK (start_observed IN (0, 1)),
            started_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL,
            lease_expires_at TEXT NOT NULL,
            ended_at TEXT,
            UNIQUE (project_id, host, host_run_key)
        )",
        "INSERT INTO agent_runs_v30 (
            run_id, project_id, issue_number, host, host_run_key, host_session_id,
            parent_run_id, kind, phase, outcome, end_reason, display_label, summary,
            revision, start_observed, started_at, last_seen_at, lease_expires_at, ended_at
         )
         SELECT run_id, project_id, issue_number, host, host_run_key, host_session_id,
                parent_run_id, kind, phase, outcome, end_reason, display_label, summary,
                revision, start_observed, started_at, last_seen_at, lease_expires_at, ended_at
         FROM agent_runs",
        "DROP TABLE agent_runs",
        "ALTER TABLE agent_runs_v30 RENAME TO agent_runs",
        "CREATE INDEX idx_agent_runs_project_issue_latest
         ON agent_runs (project_id, issue_number, last_seen_at DESC, run_id DESC)",
        "CREATE INDEX idx_agent_runs_running_lease
         ON agent_runs (phase, lease_expires_at)",
        "CREATE INDEX idx_agent_runs_project_session
         ON agent_runs (project_id, host, host_session_id, phase)",
    ] {
        sqlx::query(statement).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *connection)
        .await?;
    Ok(())
}

/// Widen the project_agent_adapters adapter CHECK constraint to accept the
/// opencode plugin-hook integration. SQLite cannot alter a CHECK constraint
/// in place, so the table is rebuilt. The table is a child of project_bindings
/// and is referenced by nothing else, so the rebuild is FK-safe on a single
/// transaction.
pub(crate) async fn migrate_local_schema_30_to_31(pool: &SqlitePool) -> Result<(), DaemonError> {
    let mut tx = pool.begin().await?;
    let table_sql: Option<String> = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'project_agent_adapters'",
    )
    .fetch_optional(&mut *tx)
    .await?;
    if table_sql
        .as_deref()
        .is_some_and(|sql| sql.contains("'opencode'"))
    {
        tx.commit().await?;
        return Ok(());
    }
    if table_sql.is_none() {
        // A library that never ran schema 20 has no project_agent_adapters
        // table; agent_adapter::migrate creates the widened shape.
        tx.commit().await?;
        return Ok(());
    }
    for statement in [
        "CREATE TABLE project_agent_adapters_v31 (
            server_url TEXT NOT NULL,
            workspace_root TEXT NOT NULL,
            project_id TEXT NOT NULL,
            adapter TEXT NOT NULL CHECK (adapter IN ('codex', 'claude-code', 'opencode')),
            revision BIGINT NOT NULL CHECK (revision > 0),
            manifest_json TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            PRIMARY KEY (server_url, workspace_root, adapter),
            FOREIGN KEY (server_url, workspace_root)
                REFERENCES project_bindings(server_url, workspace_root)
                ON DELETE CASCADE
        )",
        "INSERT INTO project_agent_adapters_v31 (
            server_url, workspace_root, project_id, adapter, revision,
            manifest_json, created_at, updated_at
         )
         SELECT server_url, workspace_root, project_id, adapter, revision,
                manifest_json, created_at, updated_at
         FROM project_agent_adapters",
        "DROP TABLE project_agent_adapters",
        "ALTER TABLE project_agent_adapters_v31 RENAME TO project_agent_adapters",
        "CREATE INDEX idx_project_agent_adapters_project
         ON project_agent_adapters (server_url, project_id)",
    ] {
        sqlx::query(statement).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn migrate_local_schema_31_to_32(pool: &SqlitePool) -> Result<(), DaemonError> {
    work_tracking::migrate(pool).await
}

pub(crate) async fn migrate_local_schema_32_to_33(pool: &SqlitePool) -> Result<(), DaemonError> {
    work_tracking::migrate(pool).await
}

pub(crate) async fn migrate_local_schema_33_to_34(pool: &SqlitePool) -> Result<(), DaemonError> {
    work_tracking::migrate(pool).await
}

pub(crate) async fn migrate_local_schema_34_to_35(pool: &SqlitePool) -> Result<(), DaemonError> {
    sqlx::query("UPDATE agent_runs SET host = 'opencode' WHERE host = 'manual'")
        .execute(pool)
        .await?;
    Ok(())
}

/// Adds the `dsh` AgentRun host to the agent_runs CHECK constraint. The
/// CHECK is enforced at the table level, so the table is rebuilt (copy ->
/// drop -> rename) exactly like the v28 rebuild that added `opencode`.
///
/// Unlike the v28 rebuild, later tables hold foreign keys to agent_runs.
/// With FK enforcement on, DROP TABLE's implicit DELETE either violates
/// those references or (for ON DELETE CASCADE children) silently deletes
/// child rows. The rebuild therefore follows SQLite's documented
/// table-rebuild procedure: disable foreign_keys on the same connection,
/// rebuild inside a transaction, then re-enable — all on one pooled
/// connection so the pragma is guaranteed to apply.
pub(crate) async fn migrate_local_schema_35_to_36(pool: &SqlitePool) -> Result<(), DaemonError> {
    let mut connection = pool.acquire().await?;
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *connection)
        .await?;
    let result = async {
        let mut tx = connection.begin().await?;
        let table_sql: Option<String> = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'agent_runs'",
        )
        .fetch_optional(&mut *tx)
        .await?;
        if table_sql
            .as_deref()
            .is_some_and(|sql| sql.contains("'dsh'"))
        {
            tx.commit().await?;
            return Ok(());
        }
        let create_table = "CREATE TABLE agent_runs (
            run_id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            issue_number BIGINT CHECK (issue_number IS NULL OR issue_number > 0),
            host TEXT NOT NULL CHECK (host IN ('codex', 'claude-code', 'zed', 'manual', 'opencode', 'dsh')),
            host_run_key TEXT NOT NULL,
            host_session_id TEXT,
            parent_run_id TEXT REFERENCES agent_runs(run_id),
            kind TEXT NOT NULL CHECK (kind IN ('root', 'subagent')),
            phase TEXT NOT NULL CHECK (phase IN ('running', 'ended')),
            outcome TEXT CHECK (outcome IN ('completed', 'blocked', 'failed', 'cancelled', 'unknown')),
            end_reason TEXT,
            display_label TEXT,
            summary TEXT,
            revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
            start_observed INTEGER NOT NULL DEFAULT 1 CHECK (start_observed IN (0, 1)),
            started_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL,
            lease_expires_at TEXT NOT NULL,
            ended_at TEXT,
            UNIQUE (project_id, host, host_run_key)
        )";
        if table_sql.is_none() {
            // No agent_runs table at all: create the v36 shape directly.
            for statement in [
                create_table,
                "CREATE INDEX idx_agent_runs_project_issue_latest
                 ON agent_runs (project_id, issue_number, last_seen_at DESC, run_id DESC)",
                "CREATE INDEX idx_agent_runs_running_lease
                 ON agent_runs (phase, lease_expires_at)",
                "CREATE INDEX idx_agent_runs_project_session
                 ON agent_runs (project_id, host, host_session_id, phase)",
            ] {
                sqlx::query(statement).execute(&mut *tx).await?;
            }
            tx.commit().await?;
            return Ok(());
        }
        // Rebuild: copy into a staging table with the new CHECK, drop the
        // original, rename the staging table into place.
        let staging = "CREATE TABLE agent_runs_v36 (
            run_id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            issue_number BIGINT CHECK (issue_number IS NULL OR issue_number > 0),
            host TEXT NOT NULL CHECK (host IN ('codex', 'claude-code', 'zed', 'manual', 'opencode', 'dsh')),
            host_run_key TEXT NOT NULL,
            host_session_id TEXT,
            parent_run_id TEXT REFERENCES agent_runs(run_id),
            kind TEXT NOT NULL CHECK (kind IN ('root', 'subagent')),
            phase TEXT NOT NULL CHECK (phase IN ('running', 'ended')),
            outcome TEXT CHECK (outcome IN ('completed', 'blocked', 'failed', 'cancelled', 'unknown')),
            end_reason TEXT,
            display_label TEXT,
            summary TEXT,
            revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
            start_observed INTEGER NOT NULL DEFAULT 1 CHECK (start_observed IN (0, 1)),
            started_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL,
            lease_expires_at TEXT NOT NULL,
            ended_at TEXT,
            UNIQUE (project_id, host, host_run_key)
        )";
        for statement in [
            staging,
            "INSERT INTO agent_runs_v36 (
                run_id, project_id, issue_number, host, host_run_key, host_session_id,
                parent_run_id, kind, phase, outcome, end_reason, display_label, summary,
                revision, start_observed, started_at, last_seen_at, lease_expires_at, ended_at
             )
             SELECT run_id, project_id, issue_number, host, host_run_key, host_session_id,
                    parent_run_id, kind, phase, outcome, end_reason, display_label, summary,
                    revision, start_observed, started_at, last_seen_at, lease_expires_at, ended_at
             FROM agent_runs",
            "DROP TABLE agent_runs",
            "ALTER TABLE agent_runs_v36 RENAME TO agent_runs",
            "CREATE INDEX idx_agent_runs_project_issue_latest
             ON agent_runs (project_id, issue_number, last_seen_at DESC, run_id DESC)",
            "CREATE INDEX idx_agent_runs_running_lease
             ON agent_runs (phase, lease_expires_at)",
            "CREATE INDEX idx_agent_runs_project_session
             ON agent_runs (project_id, host, host_session_id, phase)",
        ] {
            sqlx::query(statement).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    .await;
    // The pragma is connection-scoped: restore it even on failure so the
    // pooled connection never leaks with FK enforcement disabled.
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *connection)
        .await?;
    result
}

pub(crate) async fn migrate_local_schema_13_to_14(pool: &SqlitePool) -> Result<(), DaemonError> {
    let mut tx = pool.begin().await?;
    for statement in [
        "DROP INDEX IF EXISTS idx_local_drafts_target_id",
        "DROP INDEX IF EXISTS idx_local_drafts_server_draft_id",
        "DROP INDEX IF EXISTS idx_local_draft_operations_server_operation_id",
        "DROP INDEX IF EXISTS idx_local_draft_operations_sync_status",
        "ALTER TABLE local_draft_operations RENAME TO local_draft_operations_v13",
        "ALTER TABLE local_drafts RENAME TO local_drafts_v13",
        "CREATE TABLE local_drafts (
            draft_id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            server_draft_id TEXT,
            server_version BIGINT NOT NULL DEFAULT 0,
            base_commit_id TEXT,
            resource_scope TEXT NOT NULL CHECK (resource_scope IN ('org', 'project')),
            resource_kind TEXT NOT NULL CHECK (resource_kind IN ('context', 'rule', 'workflow', 'memory')),
            target_id TEXT,
            path TEXT,
            conflict_base_commit_id TEXT,
            conflict_current_commit_id TEXT,
            conflicted_at TEXT,
            status TEXT NOT NULL CHECK (status IN ('open', 'submitted', 'discarded', 'conflicted', 'merged')) DEFAULT 'open',
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
        "INSERT INTO local_drafts (
            draft_id, project_id, server_draft_id, server_version, base_commit_id,
            resource_scope, resource_kind, target_id, path, conflict_base_commit_id,
            conflict_current_commit_id, conflicted_at, status, created_at, updated_at
         )
         SELECT
            draft_id, project_id, server_draft_id, server_version, base_commit_id,
            resource_scope, resource_kind, target_id, path, conflict_base_commit_id,
            conflict_current_commit_id, conflicted_at, status, created_at, updated_at
         FROM local_drafts_v13
         WHERE resource_kind <> 'metaprompt'",
        "CREATE TABLE local_draft_operations (
            local_operation_id TEXT PRIMARY KEY,
            draft_id TEXT NOT NULL REFERENCES local_drafts(draft_id) ON DELETE CASCADE,
            server_operation_id TEXT,
            resource_kind TEXT NOT NULL CHECK (resource_kind IN ('context', 'rule', 'workflow', 'memory')),
            operation_json TEXT NOT NULL,
            source TEXT NOT NULL CHECK (source IN ('desktop', 'cli', 'mcp_store', 'server')),
            sync_status TEXT NOT NULL CHECK (sync_status IN ('queued', 'syncing', 'retrying', 'synced', 'failed')),
            last_error TEXT,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
        "INSERT INTO local_draft_operations (
            local_operation_id, draft_id, server_operation_id, resource_kind,
            operation_json, source, sync_status, last_error, created_at, updated_at
         )
         SELECT
            operation.local_operation_id, operation.draft_id, operation.server_operation_id,
            operation.resource_kind, operation.operation_json, operation.source,
            operation.sync_status, operation.last_error, operation.created_at, operation.updated_at
         FROM local_draft_operations_v13 AS operation
         JOIN local_drafts AS draft ON draft.draft_id = operation.draft_id
         WHERE operation.resource_kind <> 'metaprompt'",
        "DROP TABLE local_draft_operations_v13",
        "DROP TABLE local_drafts_v13",
        "DELETE FROM daemon_meta WHERE key = 'draft_events_cursor'",
        "INSERT INTO daemon_meta (key, value)
         VALUES ('memory_cache_reset_required', '1')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    ] {
        sqlx::query(statement).execute(&mut *tx).await?;
    }
    migrate_legacy_rule_operations(&mut tx).await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn migrate_local_schema_14_to_15(pool: &SqlitePool) -> Result<(), DaemonError> {
    let mut tx = pool.begin().await?;
    for statement in [
        "DROP INDEX IF EXISTS idx_local_drafts_target_id",
        "DROP INDEX IF EXISTS idx_local_drafts_server_draft_id",
        "DROP INDEX IF EXISTS idx_local_draft_operations_server_operation_id",
        "DROP INDEX IF EXISTS idx_local_draft_operations_sync_status",
        "ALTER TABLE local_draft_operations RENAME TO local_draft_operations_v14",
        "ALTER TABLE local_drafts RENAME TO local_drafts_v14",
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
            resource_kind TEXT NOT NULL CHECK (resource_kind IN ('context', 'rule', 'workflow', 'memory')),
            target_id TEXT,
            path TEXT,
            status TEXT NOT NULL CHECK (status IN ('open', 'submitted', 'merged', 'discarded')) DEFAULT 'open',
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
        "INSERT INTO local_drafts (
            draft_id, project_id, server_draft_id, server_version, base_commit_id,
            current_commit_id, freshness, reconciliation, reconciliation_candidate_id,
            resource_scope, resource_kind, target_id, path, status, created_at, updated_at
         )
         SELECT
            draft_id, project_id, server_draft_id, server_version, base_commit_id,
            CASE WHEN status = 'conflicted' THEN conflict_current_commit_id ELSE base_commit_id END,
            CASE WHEN status = 'conflicted' THEN 'behind' ELSE 'current' END,
            'unknown', NULL,
            resource_scope, resource_kind, target_id, path,
            CASE WHEN status = 'conflicted' THEN 'submitted' ELSE status END,
            created_at, updated_at
         FROM local_drafts_v14",
        "CREATE TABLE local_draft_operations (
            local_operation_id TEXT PRIMARY KEY,
            draft_id TEXT NOT NULL REFERENCES local_drafts(draft_id) ON DELETE CASCADE,
            server_operation_id TEXT,
            resource_kind TEXT NOT NULL CHECK (resource_kind IN ('context', 'rule', 'workflow', 'memory')),
            operation_json TEXT NOT NULL,
            source TEXT NOT NULL CHECK (source IN ('desktop', 'cli', 'mcp_store', 'server')),
            sync_status TEXT NOT NULL CHECK (sync_status IN ('queued', 'syncing', 'retrying', 'synced', 'failed')),
            last_error TEXT,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
        "INSERT INTO local_draft_operations (
            local_operation_id, draft_id, server_operation_id, resource_kind,
            operation_json, source, sync_status, last_error, created_at, updated_at
         )
         SELECT local_operation_id, draft_id, server_operation_id, resource_kind,
                operation_json, source, sync_status, last_error, created_at, updated_at
         FROM local_draft_operations_v14",
        "DROP TABLE local_draft_operations_v14",
        "DROP TABLE local_drafts_v14",
    ] {
        sqlx::query(statement).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn migrate_local_schema_15_to_16(pool: &SqlitePool) -> Result<(), DaemonError> {
    create_project_bindings_table(pool).await
}

pub(crate) async fn migrate_local_schema_16_to_17(pool: &SqlitePool) -> Result<(), DaemonError> {
    project_storage::migrate(pool).await
}

pub(crate) async fn migrate_local_schema_17_to_18(pool: &SqlitePool) -> Result<(), DaemonError> {
    retrieval_history::migrate_schema_17_to_18(pool).await
}

pub(crate) async fn migrate_local_schema_18_to_19(pool: &SqlitePool) -> Result<(), DaemonError> {
    retrieval_history::migrate_schema_18_to_19(pool).await
}

pub(crate) async fn migrate_local_schema_19_to_20(pool: &SqlitePool) -> Result<(), DaemonError> {
    agent_adapter::migrate(pool).await
}

pub(crate) async fn create_project_bindings_table(pool: &SqlitePool) -> Result<(), DaemonError> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS project_bindings (
            server_url TEXT NOT NULL,
            workspace_root TEXT NOT NULL,
            project_id TEXT NOT NULL,
            revision BIGINT NOT NULL CHECK (revision > 0),
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            PRIMARY KEY (server_url, workspace_root)
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_project_bindings_project
         ON project_bindings (server_url, project_id)",
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn migrate_legacy_rule_operations(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<(), DaemonError> {
    let rows = sqlx::query(
        "SELECT operation.local_operation_id, operation.operation_json, draft.path
         FROM local_draft_operations AS operation
         JOIN local_drafts AS draft ON draft.draft_id = operation.draft_id
         WHERE operation.resource_kind = 'rule'",
    )
    .fetch_all(&mut **tx)
    .await?;
    for row in rows {
        let operation_id: String = row.try_get("local_operation_id")?;
        let operation_json: String = row.try_get("operation_json")?;
        let fallback_path: Option<String> = row.try_get("path")?;
        let Some(operation_json) =
            flatten_legacy_rule_operation(&operation_json, fallback_path.as_deref())?
        else {
            continue;
        };
        sqlx::query(
            "UPDATE local_draft_operations SET operation_json = $2
             WHERE local_operation_id = $1",
        )
        .bind(operation_id)
        .bind(operation_json)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn flatten_legacy_rule_operation(
    operation_json: &str,
    fallback_path: Option<&str>,
) -> Result<Option<String>, DaemonError> {
    let mut operation: serde_json::Value = serde_json::from_str(operation_json)?;
    let mut changed = false;
    for action in ["create", "update"] {
        let Some(action_value) = operation
            .get_mut(action)
            .and_then(|value| value.as_object_mut())
        else {
            continue;
        };
        let fallback_name = action_value
            .get("path")
            .and_then(|value| value.as_str())
            .or(fallback_path)
            .and_then(legacy_rule_name_from_path)
            .unwrap_or("Rule")
            .to_owned();
        let Some(content) = action_value
            .get_mut("content")
            .and_then(|value| value.as_object_mut())
        else {
            continue;
        };
        if content.get("kind").and_then(|value| value.as_str()) != Some("rule") {
            continue;
        }
        let Some(constraint) = content
            .get("constraint")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned)
        else {
            continue;
        };
        let name = content
            .get("name")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&fallback_name)
            .to_owned();
        let applies_when = content
            .get("applies_when")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_owned();
        let tags = content
            .get("tags")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(ToOwned::to_owned))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        *content = serde_json::Map::from_iter([
            ("kind".to_owned(), json!("rule")),
            (
                "content".to_owned(),
                json!(render_legacy_rule_markdown(
                    &name,
                    &applies_when,
                    &constraint,
                    &tags,
                )),
            ),
        ]);
        changed = true;
    }
    if changed {
        Ok(Some(serde_json::to_string(&operation)?))
    } else {
        Ok(None)
    }
}

fn legacy_rule_name_from_path(path: &str) -> Option<&str> {
    Path::new(path).file_stem().and_then(|name| name.to_str())
}

fn render_legacy_rule_markdown(
    name: &str,
    applies_when: &str,
    constraint: &str,
    tags: &[String],
) -> String {
    format!(
        "# {name}\n\n## Applies when\n\n{applies_when}\n\n## Constraint\n\n{constraint}\n\nTags: {}",
        if tags.is_empty() {
            "None".to_owned()
        } else {
            tags.join(", ")
        }
    )
}

pub(crate) async fn reset_memory_cache_if_required(
    pool: &SqlitePool,
    cache_dir: &Path,
) -> Result<(), DaemonError> {
    let required: Option<String> =
        sqlx::query_scalar("SELECT value FROM daemon_meta WHERE key = $1")
            .bind(META_MEMORY_CACHE_RESET_REQUIRED)
            .fetch_optional(pool)
            .await?;
    if required.as_deref() != Some("1") {
        return Ok(());
    }

    let mut tx = pool.begin().await?;
    for statement in [
        "DELETE FROM cached_refs",
        "DELETE FROM cached_commits",
        "DELETE FROM cached_trees",
        "DELETE FROM cached_blobs",
    ] {
        sqlx::query(statement).execute(&mut *tx).await?;
    }
    tx.commit().await?;

    match std::fs::remove_dir_all(cache_dir.join("projects")) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    sqlx::query("DELETE FROM daemon_meta WHERE key = $1")
        .bind(META_MEMORY_CACHE_RESET_REQUIRED)
        .execute(pool)
        .await?;
    Ok(())
}

pub(crate) async fn current_schema_version(pool: &SqlitePool) -> Result<i64, DaemonError> {
    let value: Option<String> =
        sqlx::query_scalar("SELECT value FROM daemon_meta WHERE key = 'schema_version'")
            .fetch_optional(pool)
            .await?;
    Ok(value
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or_default())
}

pub(crate) async fn load_or_create_installation_id(
    pool: &SqlitePool,
) -> Result<String, DaemonError> {
    if let Some(value) = sqlx::query_scalar::<_, String>(
        "SELECT value FROM daemon_meta WHERE key = 'daemon_installation_id'",
    )
    .fetch_optional(pool)
    .await?
    .filter(|value| !value.trim().is_empty())
    {
        return Ok(value);
    }

    let value = format!("daemon_{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO daemon_meta (key, value)
         VALUES ('daemon_installation_id', $1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(&value)
    .execute(pool)
    .await?;
    Ok(value)
}

pub(crate) async fn load_project_config(
    pool: &SqlitePool,
    defaults: &ProjectConfig,
    credentials: Option<ServerCredentials>,
) -> Result<RuntimeProjectConfig, DaemonError> {
    let server_url = load_meta_value(pool, "project_config_server_url")
        .await?
        .unwrap_or_else(|| defaults.server_url.clone());
    let project_id = load_meta_value(pool, "project_config_project_id")
        .await?
        .or_else(|| defaults.project_id.clone());
    let memory_guidelines_path = load_meta_value(pool, "project_config_memory_guidelines_path")
        .await?
        .or_else(|| defaults.memory_guidelines_path.clone());
    let credentials = credentials.filter(|credentials| credentials.server_url == server_url);
    Ok(RuntimeProjectConfig {
        server_url,
        project_id,
        memory_guidelines_path,
        access_token: credentials
            .as_ref()
            .map(|credentials| credentials.access_token.clone()),
        refresh_token: credentials.and_then(|credentials| credentials.refresh_token),
    })
}

pub(crate) async fn save_project_metadata(
    pool: &SqlitePool,
    config: &ProjectConfig,
) -> Result<(), DaemonError> {
    let mut tx = pool.begin().await?;
    upsert_meta_value(
        &mut tx,
        "project_config_server_url",
        Some(&config.server_url),
    )
    .await?;
    upsert_meta_value(
        &mut tx,
        "project_config_project_id",
        config.project_id.as_deref(),
    )
    .await?;
    upsert_meta_value(
        &mut tx,
        "project_config_memory_guidelines_path",
        config.memory_guidelines_path.as_deref(),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn load_server_credentials(
    credential_store: Arc<dyn CredentialStore>,
) -> Result<Option<ServerCredentials>, DaemonError> {
    let credentials = tokio::task::spawn_blocking(move || credential_store.load())
        .await
        .map_err(|error| {
            DaemonError::CredentialStore(CredentialStoreError::new(format!(
                "credential worker failed: {error}"
            )))
        })??;
    Ok(credentials)
}

pub(crate) async fn replace_server_credentials(
    credential_store: Arc<dyn CredentialStore>,
    credentials: Option<ServerCredentials>,
) -> Result<(), DaemonError> {
    tokio::task::spawn_blocking(move || match credentials {
        Some(credentials) => credential_store.replace(&credentials),
        None => credential_store.clear(),
    })
    .await
    .map_err(|error| {
        DaemonError::CredentialStore(CredentialStoreError::new(format!(
            "credential worker failed: {error}"
        )))
    })??;
    Ok(())
}

pub(crate) async fn load_meta_value(
    pool: &SqlitePool,
    key: &str,
) -> Result<Option<String>, DaemonError> {
    Ok(
        sqlx::query_scalar::<_, String>("SELECT value FROM daemon_meta WHERE key = $1")
            .bind(key)
            .fetch_optional(pool)
            .await?
            .and_then(non_empty_string),
    )
}

pub(crate) async fn upsert_meta_timestamp(pool: &SqlitePool, key: &str) -> Result<(), DaemonError> {
    sqlx::query(
        "INSERT INTO daemon_meta (key, value)
         VALUES ($1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn upsert_meta_value(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &str,
    value: Option<&str>,
) -> Result<(), DaemonError> {
    if let Some(value) = value.and_then(|value| non_empty_string(value.to_owned())) {
        sqlx::query(
            "INSERT INTO daemon_meta (key, value)
             VALUES ($1, $2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&mut **tx)
        .await?;
    } else {
        sqlx::query("DELETE FROM daemon_meta WHERE key = $1")
            .bind(key)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

pub(crate) async fn migrate_local_schema_36_to_37(pool: &SqlitePool) -> Result<(), DaemonError> {
    let mut connection = pool.acquire().await?;
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *connection)
        .await?;
    let result = async {
        let mut tx = connection.begin().await?;

        // local_drafts: widen resource_kind so the unified Memory kind is
        // accepted; existing rows keep their legacy values untouched.
        let drafts_sql: Option<String> = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'local_drafts'",
        )
        .fetch_optional(&mut *tx)
        .await?;
        if drafts_sql.is_none() {
            // No local_drafts table (e.g. minimal migration seeds); nothing
            // to rebuild.
        } else if drafts_sql.as_deref().is_some_and(|sql| sql.contains("'memory'")) {
            // Already migrated.
        } else {
            sqlx::query(
                "CREATE TABLE local_drafts_v37 (
                    draft_id TEXT PRIMARY KEY,
                    project_id TEXT NOT NULL,
                    server_draft_id TEXT,
                    server_version BIGINT NOT NULL DEFAULT 0,
                    base_commit_id TEXT,
                    current_commit_id TEXT,
                    freshness TEXT NOT NULL CHECK (freshness IN ('current', 'behind')) DEFAULT 'current',
                    has_upstream_resource_changes INTEGER NOT NULL CHECK (has_upstream_resource_changes IN (0, 1)) DEFAULT 0,
                    reconciliation TEXT NOT NULL CHECK (reconciliation IN ('unknown', 'clean', 'conflicts')) DEFAULT 'unknown',
                    reconciliation_candidate_id TEXT,
                    resource_scope TEXT NOT NULL CHECK (resource_scope IN ('org', 'project')),
                    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('context', 'rule', 'workflow', 'memory')),
                    target_id TEXT,
                    path TEXT,
                    status TEXT NOT NULL CHECK (status IN ('open', 'submitted', 'merged', 'discarded')) DEFAULT 'open',
                    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                )",
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO local_drafts_v37
                 SELECT draft_id, project_id, server_draft_id, server_version, base_commit_id,
                        current_commit_id, freshness, has_upstream_resource_changes, reconciliation,
                        reconciliation_candidate_id, resource_scope, resource_kind, target_id, path,
                        status, created_at, updated_at
                 FROM local_drafts",
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query("DROP TABLE local_drafts").execute(&mut *tx).await?;
            sqlx::query("ALTER TABLE local_drafts_v37 RENAME TO local_drafts")
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "CREATE INDEX IF NOT EXISTS idx_local_drafts_target_id
                 ON local_drafts (target_id)",
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_local_drafts_server_draft_id
                 ON local_drafts (server_draft_id)
                 WHERE server_draft_id IS NOT NULL",
            )
            .execute(&mut *tx)
            .await?;
        }

        // local_draft_operations: widen resource_kind the same way.
        let operations_sql: Option<String> = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'local_draft_operations'",
        )
        .fetch_optional(&mut *tx)
        .await?;
        if operations_sql.is_none() {
            // No local_draft_operations table; nothing to rebuild.
        } else if operations_sql.as_deref().is_some_and(|sql| sql.contains("'memory'")) {
            // Already migrated.
        } else {
            sqlx::query(
                "CREATE TABLE local_draft_operations_v37 (
                    local_operation_id TEXT PRIMARY KEY,
                    draft_id TEXT NOT NULL REFERENCES local_drafts(draft_id) ON DELETE CASCADE,
                    server_operation_id TEXT,
                    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('context', 'rule', 'workflow', 'memory')),
                    operation_json TEXT NOT NULL,
                    source TEXT NOT NULL CHECK (source IN ('desktop', 'cli', 'mcp_store', 'server')),
                    sync_status TEXT NOT NULL CHECK (sync_status IN ('queued', 'syncing', 'retrying', 'synced', 'failed')),
                    last_error TEXT,
                    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                )",
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO local_draft_operations_v37
                 SELECT local_operation_id, draft_id, server_operation_id, resource_kind,
                        operation_json, source, sync_status, last_error, created_at, updated_at
                 FROM local_draft_operations",
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query("DROP TABLE local_draft_operations")
                .execute(&mut *tx)
                .await?;
            sqlx::query("ALTER TABLE local_draft_operations_v37 RENAME TO local_draft_operations")
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_local_draft_operations_server_operation_id
                 ON local_draft_operations (server_operation_id)
                 WHERE server_operation_id IS NOT NULL",
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "CREATE INDEX IF NOT EXISTS idx_local_draft_operations_sync_status
                 ON local_draft_operations (sync_status)",
            )
            .execute(&mut *tx)
            .await?;
        }

        // Derived search indexes are rebuilt from the new Commit baseline,
        // so a reset marker keeps stale kind-constrained indexes from being
        // queried before the rebuild.
        sqlx::query(
            "INSERT INTO daemon_meta (key, value) VALUES ($1, '1')
             ON CONFLICT(key) DO UPDATE SET value = '1'",
        )
        .bind(META_MEMORY_CACHE_RESET_REQUIRED)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
    .await;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *connection)
        .await?;
    result
}

/// Widens the `agent_runs`, `project_agent_adapters`, and `adapter_fs_ops`
/// CHECK constraints to accept `dsh`.
pub(crate) async fn migrate_local_schema_37_to_38(pool: &SqlitePool) -> Result<(), DaemonError> {
    let mut connection = pool.acquire().await?;
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *connection)
        .await?;
    let result = async {
        let mut tx = connection.begin().await?;

        // 1. agent_runs
        let runs_sql: Option<String> = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'agent_runs'",
        )
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(sql) = runs_sql
            && !sql.contains("'dsh'")
        {
            for statement in [
                "CREATE TABLE agent_runs_v38 (
                    run_id TEXT PRIMARY KEY,
                    project_id TEXT NOT NULL,
                    issue_number BIGINT CHECK (issue_number IS NULL OR issue_number > 0),
                    host TEXT NOT NULL CHECK (host IN ('codex', 'claude-code', 'zed', 'manual', 'opencode', 'dsh')),
                    host_run_key TEXT NOT NULL,
                    host_session_id TEXT,
                    parent_run_id TEXT REFERENCES agent_runs(run_id),
                    kind TEXT NOT NULL CHECK (kind IN ('root', 'subagent')),
                    phase TEXT NOT NULL CHECK (phase IN ('running', 'ended')),
                    outcome TEXT CHECK (outcome IN ('completed', 'blocked', 'failed', 'cancelled', 'unknown')),
                    end_reason TEXT,
                    display_label TEXT,
                    summary TEXT,
                    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
                    start_observed INTEGER NOT NULL DEFAULT 1 CHECK (start_observed IN (0, 1)),
                    started_at TEXT NOT NULL,
                    last_seen_at TEXT NOT NULL,
                    lease_expires_at TEXT NOT NULL,
                    ended_at TEXT,
                    UNIQUE (project_id, host, host_run_key)
                )",
                "INSERT INTO agent_runs_v38
                 SELECT run_id, project_id, issue_number, host, host_run_key, host_session_id,
                        parent_run_id, kind, phase, outcome, end_reason, display_label, summary,
                        revision, start_observed, started_at, last_seen_at, lease_expires_at, ended_at
                 FROM agent_runs",
                "DROP TABLE agent_runs",
                "ALTER TABLE agent_runs_v38 RENAME TO agent_runs",
                "CREATE INDEX idx_agent_runs_project_issue_latest
                 ON agent_runs (project_id, issue_number, last_seen_at DESC, run_id DESC)",
                "CREATE INDEX idx_agent_runs_running_lease
                 ON agent_runs (phase, lease_expires_at)",
                "CREATE INDEX idx_agent_runs_project_session
                 ON agent_runs (project_id, host, host_session_id, phase)",
            ] {
                sqlx::query(statement).execute(&mut *tx).await?;
            }
        }

        // 2. project_agent_adapters
        let adapters_sql: Option<String> = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'project_agent_adapters'",
        )
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(sql) = adapters_sql
            && !sql.contains("'dsh'")
        {
            for statement in [
                "CREATE TABLE project_agent_adapters_v38 (
                    server_url TEXT NOT NULL,
                    workspace_root TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    adapter TEXT NOT NULL CHECK (adapter IN ('codex', 'claude-code', 'opencode', 'dsh')),
                    revision BIGINT NOT NULL CHECK (revision > 0),
                    manifest_json TEXT NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                    PRIMARY KEY (server_url, workspace_root, adapter),
                    FOREIGN KEY (server_url, workspace_root)
                        REFERENCES project_bindings(server_url, workspace_root)
                        ON DELETE CASCADE
                )",
                "INSERT INTO project_agent_adapters_v38
                 SELECT server_url, workspace_root, project_id, adapter, revision,
                        manifest_json, created_at, updated_at
                 FROM project_agent_adapters",
                "DROP TABLE project_agent_adapters",
                "ALTER TABLE project_agent_adapters_v38 RENAME TO project_agent_adapters",
                "CREATE INDEX idx_project_agent_adapters_project
                 ON project_agent_adapters (server_url, project_id)",
            ] {
                sqlx::query(statement).execute(&mut *tx).await?;
            }
        }

        // 3. adapter_fs_ops
        let fs_ops_sql: Option<String> = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'adapter_fs_ops'",
        )
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(sql) = fs_ops_sql
            && !sql.contains("'dsh'")
        {
            for statement in [
                "CREATE TABLE adapter_fs_ops_v38 (
                    operation_id TEXT PRIMARY KEY,
                    server_url TEXT NOT NULL,
                    workspace_root TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    adapter TEXT NOT NULL CHECK (adapter IN ('codex', 'claude-code', 'opencode', 'dsh')),
                    action TEXT NOT NULL CHECK (action IN ('install', 'remove')),
                    expected_revision BIGINT,
                    next_revision BIGINT,
                    manifest_json TEXT,
                    changes_json TEXT NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                    UNIQUE (server_url, workspace_root, adapter)
                )",
                "INSERT INTO adapter_fs_ops_v38
                 SELECT operation_id, server_url, workspace_root, project_id, adapter, action,
                        expected_revision, next_revision, manifest_json, changes_json, created_at
                 FROM adapter_fs_ops",
                "DROP TABLE adapter_fs_ops",
                "ALTER TABLE adapter_fs_ops_v38 RENAME TO adapter_fs_ops",
            ] {
                sqlx::query(statement).execute(&mut *tx).await?;
            }
        }

        tx.commit().await?;
        Ok(())
    }
    .await;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *connection)
        .await?;
    result
}

/// Widens the `agent_runs`, `project_agent_adapters`, and `adapter_fs_ops`
/// CHECK constraints to accept `antigravity`.
pub(crate) async fn migrate_local_schema_38_to_39(pool: &SqlitePool) -> Result<(), DaemonError> {
    let mut connection = pool.acquire().await?;
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *connection)
        .await?;
    let result = async {
        let mut tx = connection.begin().await?;

        // 1. agent_runs
        let runs_sql: Option<String> = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'agent_runs'",
        )
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(sql) = runs_sql
            && !sql.contains("'antigravity'")
        {
            for statement in [
                "CREATE TABLE agent_runs_v39 (
                    run_id TEXT PRIMARY KEY,
                    project_id TEXT NOT NULL,
                    issue_number BIGINT CHECK (issue_number IS NULL OR issue_number > 0),
                    host TEXT NOT NULL CHECK (host IN ('codex', 'claude-code', 'zed', 'manual', 'opencode', 'dsh', 'antigravity')),
                    host_run_key TEXT NOT NULL,
                    host_session_id TEXT,
                    parent_run_id TEXT REFERENCES agent_runs(run_id),
                    kind TEXT NOT NULL CHECK (kind IN ('root', 'subagent')),
                    phase TEXT NOT NULL CHECK (phase IN ('running', 'ended')),
                    outcome TEXT CHECK (outcome IN ('completed', 'blocked', 'failed', 'cancelled', 'unknown')),
                    end_reason TEXT,
                    display_label TEXT,
                    summary TEXT,
                    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
                    start_observed INTEGER NOT NULL DEFAULT 1 CHECK (start_observed IN (0, 1)),
                    started_at TEXT NOT NULL,
                    last_seen_at TEXT NOT NULL,
                    lease_expires_at TEXT NOT NULL,
                    ended_at TEXT,
                    UNIQUE (project_id, host, host_run_key)
                )",
                "INSERT INTO agent_runs_v39
                 SELECT run_id, project_id, issue_number, host, host_run_key, host_session_id,
                        parent_run_id, kind, phase, outcome, end_reason, display_label, summary,
                        revision, start_observed, started_at, last_seen_at, lease_expires_at, ended_at
                 FROM agent_runs",
                "DROP TABLE agent_runs",
                "ALTER TABLE agent_runs_v39 RENAME TO agent_runs",
                "CREATE INDEX idx_agent_runs_project_issue_latest
                 ON agent_runs (project_id, issue_number, last_seen_at DESC, run_id DESC)",
                "CREATE INDEX idx_agent_runs_running_lease
                 ON agent_runs (phase, lease_expires_at)",
                "CREATE INDEX idx_agent_runs_project_session
                 ON agent_runs (project_id, host, host_session_id, phase)",
            ] {
                sqlx::query(statement).execute(&mut *tx).await?;
            }
        }

        // 2. project_agent_adapters
        let adapters_sql: Option<String> = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'project_agent_adapters'",
        )
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(sql) = adapters_sql
            && !sql.contains("'antigravity'")
        {
            for statement in [
                "CREATE TABLE project_agent_adapters_v39 (
                    server_url TEXT NOT NULL,
                    workspace_root TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    adapter TEXT NOT NULL CHECK (adapter IN ('codex', 'claude-code', 'opencode', 'dsh', 'antigravity')),
                    revision BIGINT NOT NULL CHECK (revision > 0),
                    manifest_json TEXT NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                    PRIMARY KEY (server_url, workspace_root, adapter),
                    FOREIGN KEY (server_url, workspace_root)
                        REFERENCES project_bindings(server_url, workspace_root)
                        ON DELETE CASCADE
                )",
                "INSERT INTO project_agent_adapters_v39
                 SELECT server_url, workspace_root, project_id, adapter, revision,
                        manifest_json, created_at, updated_at
                 FROM project_agent_adapters",
                "DROP TABLE project_agent_adapters",
                "ALTER TABLE project_agent_adapters_v39 RENAME TO project_agent_adapters",
                "CREATE INDEX idx_project_agent_adapters_project
                 ON project_agent_adapters (server_url, project_id)",
            ] {
                sqlx::query(statement).execute(&mut *tx).await?;
            }
        }

        // 3. adapter_fs_ops
        let fs_ops_sql: Option<String> = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'adapter_fs_ops'",
        )
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(sql) = fs_ops_sql
            && !sql.contains("'antigravity'")
        {
            for statement in [
                "CREATE TABLE adapter_fs_ops_v39 (
                    operation_id TEXT PRIMARY KEY,
                    server_url TEXT NOT NULL,
                    workspace_root TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    adapter TEXT NOT NULL CHECK (adapter IN ('codex', 'claude-code', 'opencode', 'dsh', 'antigravity')),
                    action TEXT NOT NULL CHECK (action IN ('install', 'remove')),
                    expected_revision BIGINT,
                    next_revision BIGINT,
                    manifest_json TEXT,
                    changes_json TEXT NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                    UNIQUE (server_url, workspace_root, adapter)
                )",
                "INSERT INTO adapter_fs_ops_v39
                 SELECT operation_id, server_url, workspace_root, project_id, adapter, action,
                        expected_revision, next_revision, manifest_json, changes_json, created_at
                 FROM adapter_fs_ops",
                "DROP TABLE adapter_fs_ops",
                "ALTER TABLE adapter_fs_ops_v39 RENAME TO adapter_fs_ops",
            ] {
                sqlx::query(statement).execute(&mut *tx).await?;
            }
        }

        tx.commit().await?;
        Ok(())
    }
    .await;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *connection)
        .await?;
    result
}

/// Widens persisted retrieval-history resource kinds to accept `memory`.
pub(crate) async fn migrate_local_schema_39_to_40(pool: &SqlitePool) -> Result<(), DaemonError> {
    let mut connection = pool.acquire().await?;
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *connection)
        .await?;
    let result = async {
        let mut tx = connection.begin().await?;

        let candidates_sql: Option<String> = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master
             WHERE type = 'table' AND name = 'retrieval_run_candidates'",
        )
        .fetch_optional(&mut *tx)
        .await?;
        if candidates_sql
            .as_deref()
            .is_some_and(|sql| !sql.contains("'memory'"))
        {
            for statement in [
                "CREATE TABLE retrieval_run_candidates_v40 (
                    run_id TEXT NOT NULL,
                    candidate_order BIGINT NOT NULL CHECK (candidate_order >= 0),
                    unit_key TEXT NOT NULL,
                    resource_id TEXT NOT NULL,
                    scope TEXT NOT NULL CHECK (scope IN ('org', 'project')),
                    kind TEXT NOT NULL CHECK (kind IN ('context', 'rule', 'workflow', 'memory')),
                    path TEXT NOT NULL,
                    heading_path_json TEXT NOT NULL,
                    locator_json TEXT NOT NULL,
                    content_hash TEXT NOT NULL,
                    resource_content_hash TEXT NOT NULL,
                    token_count BIGINT NOT NULL CHECK (token_count >= 0),
                    evidence_excerpt TEXT NOT NULL,
                    exact_rank BIGINT,
                    bm25_rank BIGINT,
                    bm25_score REAL,
                    vector_rank BIGINT,
                    vector_score REAL,
                    rrf_rank BIGINT,
                    rrf_score REAL,
                    reranker_rank BIGINT,
                    reranker_logit REAL,
                    reranker_relevance REAL,
                    final_rank BIGINT,
                    selected INTEGER NOT NULL CHECK (selected IN (0, 1)),
                    exclusion_reason TEXT NOT NULL CHECK (exclusion_reason IN (
                        'selected', 'below_relevance', 'overlap', 'per_resource_limit',
                        'token_budget', 'fragment_limit', 'not_reranked'
                    )),
                    delta_action TEXT CHECK (delta_action IN ('add', 'replace', 'reuse')),
                    PRIMARY KEY (run_id, unit_key)
                )",
                "INSERT INTO retrieval_run_candidates_v40
                 SELECT * FROM retrieval_run_candidates",
                "DROP TABLE retrieval_run_candidates",
                "ALTER TABLE retrieval_run_candidates_v40 RENAME TO retrieval_run_candidates",
                "CREATE INDEX idx_retrieval_candidates_run_order
                 ON retrieval_run_candidates (run_id, candidate_order)",
            ] {
                sqlx::query(statement).execute(&mut *tx).await?;
            }
        }

        let resources_sql: Option<String> = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master
             WHERE type = 'table' AND name = 'retrieval_run_resources'",
        )
        .fetch_optional(&mut *tx)
        .await?;
        if resources_sql
            .as_deref()
            .is_some_and(|sql| !sql.contains("'memory'"))
        {
            for statement in [
                "CREATE TABLE retrieval_run_resources_v40 (
                    run_id TEXT NOT NULL,
                    resource_order BIGINT NOT NULL CHECK (resource_order >= 0),
                    resource_id TEXT NOT NULL,
                    scope TEXT NOT NULL CHECK (scope IN ('org', 'project')),
                    kind TEXT NOT NULL CHECK (kind IN ('context', 'rule', 'workflow', 'memory')),
                    path TEXT NOT NULL,
                    title TEXT NOT NULL,
                    content_hash TEXT NOT NULL,
                    content_preview TEXT NOT NULL,
                    source_commit_id TEXT,
                    draft_id TEXT,
                    draft_revision TEXT,
                    PRIMARY KEY (run_id, resource_id)
                )",
                "INSERT INTO retrieval_run_resources_v40
                 SELECT * FROM retrieval_run_resources",
                "DROP TABLE retrieval_run_resources",
                "ALTER TABLE retrieval_run_resources_v40 RENAME TO retrieval_run_resources",
            ] {
                sqlx::query(statement).execute(&mut *tx).await?;
            }
        }

        let corpus_resources_sql: Option<String> = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master
             WHERE type = 'table' AND name = 'evaluation_corpus_resources'",
        )
        .fetch_optional(&mut *tx)
        .await?;
        if corpus_resources_sql
            .as_deref()
            .is_some_and(|sql| !sql.contains("'memory'"))
        {
            for statement in [
                "CREATE TABLE evaluation_corpus_resources_v40 (
                    corpus_id TEXT NOT NULL,
                    resource_order BIGINT NOT NULL CHECK (resource_order >= 0),
                    resource_id TEXT NOT NULL,
                    scope TEXT NOT NULL CHECK (scope IN ('org', 'project')),
                    kind TEXT NOT NULL CHECK (kind IN ('context', 'rule', 'workflow', 'memory')),
                    path TEXT NOT NULL,
                    title TEXT NOT NULL,
                    content_hash TEXT NOT NULL,
                    content_preview TEXT NOT NULL,
                    source_commit_id TEXT,
                    draft_id TEXT,
                    draft_revision TEXT,
                    PRIMARY KEY (corpus_id, resource_id)
                )",
                "INSERT INTO evaluation_corpus_resources_v40
                 SELECT * FROM evaluation_corpus_resources",
                "DROP TABLE evaluation_corpus_resources",
                "ALTER TABLE evaluation_corpus_resources_v40
                 RENAME TO evaluation_corpus_resources",
            ] {
                sqlx::query(statement).execute(&mut *tx).await?;
            }
        }

        tx.commit().await?;
        Ok(())
    }
    .await;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *connection)
        .await?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn test_pool_with_v35_runs() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        // Mirror the daemon: foreign key enforcement is on.
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        // v35-shaped agent_runs: CHECK without 'dsh'.
        sqlx::query(
            "CREATE TABLE agent_runs (
                run_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                issue_number BIGINT CHECK (issue_number IS NULL OR issue_number > 0),
                host TEXT NOT NULL CHECK (host IN ('codex', 'claude-code', 'zed', 'manual', 'opencode')),
                host_run_key TEXT NOT NULL,
                host_session_id TEXT,
                parent_run_id TEXT REFERENCES agent_runs(run_id),
                kind TEXT NOT NULL CHECK (kind IN ('root', 'subagent')),
                phase TEXT NOT NULL CHECK (phase IN ('running', 'ended')),
                outcome TEXT CHECK (outcome IN ('completed', 'blocked', 'failed', 'cancelled', 'unknown')),
                end_reason TEXT,
                display_label TEXT,
                summary TEXT,
                revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
                start_observed INTEGER NOT NULL DEFAULT 1 CHECK (start_observed IN (0, 1)),
                started_at TEXT NOT NULL,
                last_seen_at TEXT NOT NULL,
                lease_expires_at TEXT NOT NULL,
                ended_at TEXT,
                UNIQUE (project_id, host, host_run_key)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agent_runs (run_id, project_id, host, host_run_key, kind, phase,
                                     started_at, last_seen_at, lease_expires_at)
             VALUES ('arun_11111111111111111111111111111111', 'prj_1', 'codex', 'root:1',
                     'root', 'running', '2026-08-14T00:00:00.000Z', '2026-08-14T00:00:00.000Z',
                     '2026-08-14T01:00:00.000Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        // A child table holding foreign keys into agent_runs, with live rows.
        sqlx::query(
            "CREATE TABLE agent_run_events (
                event_id TEXT PRIMARY KEY,
                event_fingerprint TEXT NOT NULL,
                run_id TEXT REFERENCES agent_runs(run_id) ON DELETE CASCADE,
                event_type TEXT NOT NULL,
                source TEXT NOT NULL,
                occurred_at TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agent_run_events (event_id, event_fingerprint, run_id, event_type,
                                           source, occurred_at)
             VALUES ('evt_1', 'fp_1', 'arun_11111111111111111111111111111111', 'started',
                     'hook', '2026-08-14T00:00:00.000Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn schema_35_to_36_rebuild_keeps_child_rows_and_accepts_dsh() {
        let pool = test_pool_with_v35_runs().await;

        migrate_local_schema_35_to_36(&pool).await.unwrap();

        // The pre-existing run survived the rebuild and its child row still resolves.
        let host: String = sqlx::query_scalar(
            "SELECT host FROM agent_runs WHERE run_id = 'arun_11111111111111111111111111111111'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(host, "codex");
        let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_run_events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(events, 1);

        // The new CHECK accepts 'dsh'.
        sqlx::query(
            "INSERT INTO agent_runs (run_id, project_id, host, host_run_key, kind, phase,
                                     started_at, last_seen_at, lease_expires_at)
             VALUES ('arun_22222222222222222222222222222222', 'prj_1', 'dsh', 'root:2',
                     'root', 'running', '2026-08-14T00:00:00.000Z', '2026-08-14T00:00:00.000Z',
                     '2026-08-14T01:00:00.000Z')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Re-running is a no-op.
        migrate_local_schema_35_to_36(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn schema_35_to_36_creates_table_when_missing() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        migrate_local_schema_35_to_36(&pool).await.unwrap();
        let exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'agent_runs'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(exists, 1);
    }

    #[tokio::test]
    async fn schema_38_to_39_widens_runs_and_adapters_to_antigravity() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();

        // Seed v37-shaped tables (with dsh, without antigravity).
        sqlx::query(
            "CREATE TABLE agent_runs (
                run_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                issue_number BIGINT,
                host TEXT NOT NULL CHECK (host IN ('codex', 'claude-code', 'zed', 'manual', 'opencode', 'dsh')),
                host_run_key TEXT NOT NULL,
                host_session_id TEXT,
                parent_run_id TEXT,
                kind TEXT NOT NULL CHECK (kind IN ('root', 'subagent')),
                phase TEXT NOT NULL CHECK (phase IN ('running', 'ended')),
                outcome TEXT,
                end_reason TEXT,
                display_label TEXT,
                summary TEXT,
                revision BIGINT NOT NULL DEFAULT 1,
                start_observed INTEGER NOT NULL DEFAULT 1,
                started_at TEXT NOT NULL,
                last_seen_at TEXT NOT NULL,
                lease_expires_at TEXT NOT NULL,
                ended_at TEXT,
                UNIQUE (project_id, host, host_run_key)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE project_bindings (
                server_url TEXT NOT NULL,
                workspace_root TEXT NOT NULL,
                project_id TEXT NOT NULL,
                revision BIGINT NOT NULL,
                PRIMARY KEY (server_url, workspace_root)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE project_agent_adapters (
                server_url TEXT NOT NULL,
                workspace_root TEXT NOT NULL,
                project_id TEXT NOT NULL,
                adapter TEXT NOT NULL CHECK (adapter IN ('codex', 'claude-code', 'opencode', 'dsh')),
                revision BIGINT NOT NULL,
                manifest_json TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT '',
                updated_at TEXT NOT NULL DEFAULT '',
                PRIMARY KEY (server_url, workspace_root, adapter),
                FOREIGN KEY (server_url, workspace_root)
                    REFERENCES project_bindings(server_url, workspace_root)
                    ON DELETE CASCADE
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE adapter_fs_ops (
                operation_id TEXT PRIMARY KEY,
                server_url TEXT NOT NULL,
                workspace_root TEXT NOT NULL,
                project_id TEXT NOT NULL,
                adapter TEXT NOT NULL CHECK (adapter IN ('codex', 'claude-code', 'opencode', 'dsh')),
                action TEXT NOT NULL,
                expected_revision BIGINT,
                next_revision BIGINT,
                manifest_json TEXT,
                changes_json TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT '',
                UNIQUE (server_url, workspace_root, adapter)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Seed rows
        sqlx::query(
            "INSERT INTO project_bindings VALUES ('https://app.clumsies.ai', '/tmp/repo', 'prj_1', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO project_agent_adapters VALUES ('https://app.clumsies.ai', '/tmp/repo', 'prj_1', 'dsh', 1, '{}', '', '')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agent_runs VALUES ('arun_1', 'prj_1', 1, 'dsh', 'dsh_1', NULL, NULL, 'root', 'running', NULL, NULL, NULL, NULL, 1, 1, '2026-08-16T00:00:00Z', '2026-08-16T00:00:00Z', '2026-08-16T01:00:00Z', NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        migrate_local_schema_38_to_39(&pool).await.unwrap();

        // Check that antigravity can now be inserted into all 3 tables
        sqlx::query(
            "INSERT INTO project_agent_adapters VALUES ('https://app.clumsies.ai', '/tmp/repo', 'prj_1', 'antigravity', 1, '{}', '', '')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO adapter_fs_ops VALUES ('op_1', 'https://app.clumsies.ai', '/tmp/repo', 'prj_1', 'antigravity', 'install', NULL, 1, NULL, '[]', '')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agent_runs VALUES ('arun_2', 'prj_1', 1, 'antigravity', 'turn_1', NULL, NULL, 'root', 'running', NULL, NULL, NULL, NULL, 1, 1, '2026-08-16T00:00:00Z', '2026-08-16T00:00:00Z', '2026-08-16T01:00:00Z', NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Re-running migration is idempotent
        migrate_local_schema_38_to_39(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn schema_39_to_40_preserves_retrieval_history_and_accepts_memory() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE daemon_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO daemon_meta (key, value) VALUES ('schema_version', '39')")
            .execute(&pool)
            .await
            .unwrap();
        retrieval_history::migrate(&pool).await.unwrap();

        // Recreate the three tables with their v39 kind CHECK while keeping
        // the rest of the production schema byte-for-byte equivalent.
        for table in [
            "retrieval_run_candidates",
            "retrieval_run_resources",
            "evaluation_corpus_resources",
        ] {
            let current_sql: String = sqlx::query_scalar(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = $1",
            )
            .bind(table)
            .fetch_one(&pool)
            .await
            .unwrap();
            let old_table = format!("{table}_v39");
            let stale_sql = current_sql
                .replacen(
                    &format!("CREATE TABLE {table}"),
                    &format!("CREATE TABLE {old_table}"),
                    1,
                )
                .replace(", 'memory'", "");
            sqlx::query(&stale_sql).execute(&pool).await.unwrap();
            sqlx::query(&format!("INSERT INTO {old_table} SELECT * FROM {table}"))
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query(&format!("DROP TABLE {table}"))
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query(&format!("ALTER TABLE {old_table} RENAME TO {table}"))
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query(
            "CREATE INDEX idx_retrieval_candidates_run_order
             ON retrieval_run_candidates (run_id, candidate_order)",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO retrieval_run_candidates (
                run_id, candidate_order, unit_key, resource_id, scope, kind, path,
                heading_path_json, locator_json, content_hash, resource_content_hash,
                token_count, evidence_excerpt, selected, exclusion_reason, delta_action
             ) VALUES (
                'run_old', 0, 'unit_old', 'resource_old', 'project', 'context',
                'old.md', '[]', '{}', 'unit_hash_old', 'resource_hash_old', 1,
                'old candidate', 1, 'selected', 'add'
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO retrieval_run_resources (
                run_id, resource_order, resource_id, scope, kind, path, title,
                content_hash, content_preview
             ) VALUES (
                'run_old', 0, 'resource_old', 'project', 'context', 'old.md',
                'Old resource', 'resource_hash_old', 'old resource'
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO evaluation_corpus_resources (
                corpus_id, resource_order, resource_id, scope, kind, path, title,
                content_hash, content_preview
             ) VALUES (
                'corpus_old', 0, 'resource_old', 'project', 'context', 'old.md',
                'Old resource', 'resource_hash_old', 'old resource'
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE retrieval_candidate_annotations (
                annotation_id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                unit_key TEXT NOT NULL,
                note TEXT NOT NULL,
                FOREIGN KEY (run_id, unit_key)
                    REFERENCES retrieval_run_candidates(run_id, unit_key)
                    ON DELETE CASCADE
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO retrieval_candidate_annotations
             VALUES ('annotation_old', 'run_old', 'unit_old', 'keep me')",
        )
        .execute(&pool)
        .await
        .unwrap();

        migrate_local_db(&pool).await.unwrap();

        let schema_version: String =
            sqlx::query_scalar("SELECT value FROM daemon_meta WHERE key = 'schema_version'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(schema_version, CURRENT_LOCAL_SCHEMA_VERSION.to_string());
        for table in [
            "retrieval_run_candidates",
            "retrieval_run_resources",
            "evaluation_corpus_resources",
            "retrieval_candidate_annotations",
        ] {
            let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(count, 1, "lost row from {table}");
        }
        let foreign_key_violations: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM pragma_foreign_key_check")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(foreign_key_violations, 0);
        let candidate_index: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'index' AND name = 'idx_retrieval_candidates_run_order'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(candidate_index, 1);

        sqlx::query(
            "INSERT INTO retrieval_run_candidates (
                run_id, candidate_order, unit_key, resource_id, scope, kind, path,
                heading_path_json, locator_json, content_hash, resource_content_hash,
                token_count, evidence_excerpt, selected, exclusion_reason, delta_action
             ) VALUES (
                'run_memory', 0, 'unit_memory', 'resource_memory', 'project', 'memory',
                'memory.md', '[]', '{}', 'unit_hash_memory', 'resource_hash_memory', 1,
                'memory candidate', 1, 'selected', 'add'
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO retrieval_run_resources (
                run_id, resource_order, resource_id, scope, kind, path, title,
                content_hash, content_preview
             ) VALUES (
                'run_memory', 0, 'resource_memory', 'project', 'memory', 'memory.md',
                'Memory resource', 'resource_hash_memory', 'memory resource'
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO evaluation_corpus_resources (
                corpus_id, resource_order, resource_id, scope, kind, path, title,
                content_hash, content_preview
             ) VALUES (
                'corpus_memory', 0, 'resource_memory', 'project', 'memory', 'memory.md',
                'Memory resource', 'resource_hash_memory', 'memory resource'
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
    }

    async fn test_pool_with_v37_adapter_tables() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        // Mirror the daemon: foreign key enforcement is on.
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        // v37-shaped project_bindings.
        sqlx::query(
            "CREATE TABLE project_bindings (
                server_url TEXT NOT NULL,
                workspace_root TEXT NOT NULL,
                project_id TEXT NOT NULL,
                revision BIGINT NOT NULL CHECK (revision > 0),
                created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                PRIMARY KEY (server_url, workspace_root)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        // v37-shaped adapter tables: CHECK without 'dsh'.
        sqlx::query(
            "CREATE TABLE project_agent_adapters (
                server_url TEXT NOT NULL,
                workspace_root TEXT NOT NULL,
                project_id TEXT NOT NULL,
                adapter TEXT NOT NULL CHECK (adapter IN ('codex', 'claude-code', 'opencode')),
                revision BIGINT NOT NULL CHECK (revision > 0),
                manifest_json TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                PRIMARY KEY (server_url, workspace_root, adapter),
                FOREIGN KEY (server_url, workspace_root)
                    REFERENCES project_bindings(server_url, workspace_root)
                    ON DELETE CASCADE
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE INDEX idx_project_agent_adapters_project
             ON project_agent_adapters (server_url, project_id)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE adapter_fs_ops (
                operation_id TEXT PRIMARY KEY,
                server_url TEXT NOT NULL,
                workspace_root TEXT NOT NULL,
                project_id TEXT NOT NULL,
                adapter TEXT NOT NULL CHECK (adapter IN ('codex', 'claude-code', 'opencode')),
                action TEXT NOT NULL CHECK (action IN ('install', 'remove')),
                expected_revision BIGINT,
                next_revision BIGINT,
                manifest_json TEXT,
                changes_json TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                UNIQUE (server_url, workspace_root, adapter)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Seed live rows in both tables.
        sqlx::query(
            "INSERT INTO project_bindings (server_url, workspace_root, project_id, revision)
             VALUES ('https://clumsies.example.com', '/work/repo', 'prj_1', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO project_agent_adapters (
                server_url, workspace_root, project_id, adapter, revision, manifest_json)
             VALUES ('https://clumsies.example.com', '/work/repo', 'prj_1', 'codex', 1, '{}')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO adapter_fs_ops (
                operation_id, server_url, workspace_root, project_id, adapter, action,
                expected_revision, next_revision, manifest_json, changes_json)
             VALUES ('op_1', 'https://clumsies.example.com', '/work/repo', 'prj_1', 'codex',
                     'install', NULL, 1, NULL, '[]')",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn schema_37_to_38_widens_both_adapter_tables_and_preserves_rows() {
        let pool = test_pool_with_v37_adapter_tables().await;

        migrate_local_schema_37_to_38(&pool).await.unwrap();

        // Pre-existing rows survived both rebuilds.
        let adapter: String = sqlx::query_scalar(
            "SELECT adapter FROM project_agent_adapters
             WHERE server_url = 'https://clumsies.example.com' AND workspace_root = '/work/repo'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(adapter, "codex");
        let ops: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM adapter_fs_ops")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(ops, 1);

        // Both widened CHECK constraints now accept 'dsh'.
        sqlx::query(
            "INSERT INTO project_agent_adapters (
                server_url, workspace_root, project_id, adapter, revision, manifest_json)
             VALUES ('https://clumsies.example.com', '/work/repo', 'prj_1', 'dsh', 1, '{}')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO adapter_fs_ops (
                operation_id, server_url, workspace_root, project_id, adapter, action,
                expected_revision, next_revision, manifest_json, changes_json)
             VALUES ('op_2', 'https://clumsies.example.com', '/work/repo', 'prj_1', 'dsh',
                     'install', NULL, 1, NULL, '[]')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // The project index survived the rebuild.
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'index' AND name = 'idx_project_agent_adapters_project'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        // Re-running is a no-op.
        migrate_local_schema_37_to_38(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn schema_37_to_38_widens_only_the_stale_table() {
        let pool = test_pool_with_v37_adapter_tables().await;
        // Manually widen project_agent_adapters; only adapter_fs_ops stays stale.
        sqlx::query(
            "CREATE TABLE project_agent_adapters_v38 (
                server_url TEXT NOT NULL,
                workspace_root TEXT NOT NULL,
                project_id TEXT NOT NULL,
                adapter TEXT NOT NULL CHECK (adapter IN ('codex', 'claude-code', 'opencode', 'dsh')),
                revision BIGINT NOT NULL CHECK (revision > 0),
                manifest_json TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                PRIMARY KEY (server_url, workspace_root, adapter),
                FOREIGN KEY (server_url, workspace_root)
                    REFERENCES project_bindings(server_url, workspace_root)
                    ON DELETE CASCADE
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO project_agent_adapters_v38 SELECT * FROM project_agent_adapters")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DROP TABLE project_agent_adapters")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("ALTER TABLE project_agent_adapters_v38 RENAME TO project_agent_adapters")
            .execute(&pool)
            .await
            .unwrap();

        migrate_local_schema_37_to_38(&pool).await.unwrap();

        // The stale fs-ops table is now widened too.
        sqlx::query(
            "INSERT INTO adapter_fs_ops (
                operation_id, server_url, workspace_root, project_id, adapter, action,
                expected_revision, next_revision, manifest_json, changes_json)
             VALUES ('op_2', 'https://clumsies.example.com', '/work/repo', 'prj_1', 'dsh',
                     'install', NULL, 1, NULL, '[]')",
        )
        .execute(&pool)
        .await
        .unwrap();
    }
}
