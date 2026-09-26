use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Duration;

use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool, Transaction};

use super::chunker::build_units;
use super::{
    CHUNKER_VERSION, DaemonError, EffectiveMemory, PARSER_VERSION, RANKING_CONFIG_VERSION,
    RetrievalUnit, SearchFailure, SearchModels, SourceResource,
};

// Schema 7 guarantees search_resources carries the description column and the
// widened kind CHECK. Version 6 was written in two flavors: before commit
// 232eaac the column and the 'memory' kind were missing from the CREATE TABLE
// without a version bump, so rebuilds below are keyed on column presence
// rather than the version marker alone.
const PROJECT_INDEX_SCHEMA_VERSION: i64 = 7;
pub(super) const VECTOR_INPUT_VERSION: &str = "search-passage.v1:fastembed-prefix=passage";

#[derive(Clone, Debug)]
pub(super) struct BuiltUnit {
    pub(super) resource_index: usize,
    pub(super) unit: RetrievalUnit,
    vector: Vec<f32>,
    vector_input_hash: String,
}

pub(super) const INDEX_EMBED_BATCH_SIZE: usize = 32;
const VECTOR_CACHE_RETAIN_UNUSED: i64 = 4_096;

#[derive(Debug)]
pub(super) struct PreparedIndex {
    pub(super) project_id: String,
    pub(super) effective_hash: String,
    pub(super) revision_id: String,
    pub(super) model_revision: String,
    pub(super) embedding_revision: String,
    pub(super) dimensions: usize,
    units: Vec<BuiltUnit>,
}

#[derive(Debug)]
pub(super) enum PrepareIndexOutcome {
    AlreadyReady(String),
    Prepared(PreparedIndex),
    Superseded,
}

pub(crate) async fn connect_project_index(path: &Path) -> Result<SqlitePool, DaemonError> {
    if let Some(parent) = path.parent() {
        crate::project_storage::ensure_private_directory(parent)?;
    }
    let options = SqliteConnectOptions::from_str(&path.display().to_string())?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5))
        .synchronous(SqliteSynchronous::Normal);
    let pool = SqlitePoolOptions::new()
        .max_connections(3)
        .connect_with(options)
        .await?;
    // Project databases can be opened by activation, status, storage, and the
    // background scheduler concurrently. Only the resident daemon owns them,
    // so one process-wide migration gate makes schema inspection + ALTER an
    // indivisible operation without adding a cross-process compatibility path.
    let migration_lock = PROJECT_INDEX_MIGRATION_LOCK.get_or_init(|| tokio::sync::Mutex::new(()));
    let _migration_guard = migration_lock.lock().await;
    migrate_project_index(&pool).await?;
    crate::project_storage::secure_managed_tree(path.parent().unwrap_or(path))?;
    Ok(pool)
}

static PROJECT_INDEX_MIGRATION_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

async fn migrate_project_index(pool: &SqlitePool) -> Result<(), DaemonError> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS search_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    let existing: Option<String> =
        sqlx::query_scalar("SELECT value FROM search_meta WHERE key = 'schema_version'")
            .fetch_optional(pool)
            .await?;
    let existing_version = existing
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok());
    let partial_schema_without_version = existing.is_none()
        && sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type IN ('table', 'view')
               AND name IN (
                 'search_revisions', 'search_resources', 'search_units',
                 'search_units_fts', 'search_heads', 'search_vector_cache'
               )",
        )
        .fetch_one(pool)
        .await?
            > 0;
    if partial_schema_without_version
        || (existing.is_some()
            && !matches!(
                existing_version,
                Some(3) | Some(4) | Some(5) | Some(PROJECT_INDEX_SCHEMA_VERSION)
            ))
    {
        let mut tx = pool.begin().await?;
        for statement in [
            "DROP TABLE IF EXISTS search_units_fts",
            "DROP TABLE IF EXISTS search_heads",
            "DROP TABLE IF EXISTS search_units",
            "DROP TABLE IF EXISTS search_resources",
            "DROP TABLE IF EXISTS search_revisions",
            "DROP TABLE IF EXISTS search_vector_cache",
        ] {
            sqlx::query(statement).execute(&mut *tx).await?;
        }
        tx.commit().await?;
    }
    if matches!(existing_version, Some(3) | Some(4) | Some(5)) {
        let mut tx = pool.begin().await?;
        if existing_version == Some(3) {
            sqlx::query("ALTER TABLE search_revisions ADD COLUMN embedding_revision TEXT")
                .execute(&mut *tx)
                .await?;
            sqlx::query("ALTER TABLE search_units ADD COLUMN vector_input_hash TEXT")
                .execute(&mut *tx)
                .await?;
        }
        if matches!(existing_version, Some(3) | Some(4)) {
            sqlx::query("ALTER TABLE search_revisions ADD COLUMN chunker_version TEXT")
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("ALTER TABLE search_revisions ADD COLUMN dimensions BIGINT")
            .execute(&mut *tx)
            .await?;
        create_vector_cache(&mut tx).await?;
        sqlx::query(
            "INSERT INTO search_meta (key, value) VALUES ('schema_version', $1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(PROJECT_INDEX_SCHEMA_VERSION.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
    }
    // Pre-232eaac databases carry schema_version 6 while search_resources is
    // missing the description column and still checks kind against
    // ('context', 'rule', 'workflow'); older schemas (3-5) never had the
    // column either. Adding the column alone cannot repair a restrictive
    // kind CHECK because SQLite cannot alter a CHECK constraint, so those
    // index tables are rebuilt here and the next background build recreates
    // the index from the current generation with the unified Memory schema.
    // Tables without a restrictive kind CHECK only need the column added
    // and keep their ready heads. Fresh databases are untouched because the
    // CREATE TABLE below already includes the column and the widened CHECK.
    let has_search_resources: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'search_resources'",
    )
    .fetch_one(pool)
    .await?;
    let has_description_column: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('search_resources') WHERE name = 'description'",
    )
    .fetch_one(pool)
    .await?;
    if has_search_resources > 0 && has_description_column == 0 {
        let create_sql: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'search_resources'",
        )
        .fetch_one(pool)
        .await?;
        if create_sql.contains("kind IN") && !create_sql.contains("'memory'") {
            let mut tx = pool.begin().await?;
            for statement in [
                "DROP TABLE IF EXISTS search_units_fts",
                "DROP TABLE IF EXISTS search_heads",
                "DROP TABLE IF EXISTS search_units",
                "DROP TABLE IF EXISTS search_resources",
                "DROP TABLE IF EXISTS search_revisions",
                "DROP TABLE IF EXISTS search_vector_cache",
            ] {
                sqlx::query(statement).execute(&mut *tx).await?;
            }
            tx.commit().await?;
        } else {
            sqlx::query(
                "ALTER TABLE search_resources ADD COLUMN description TEXT NOT NULL DEFAULT ''",
            )
            .execute(pool)
            .await?;
        }
    }
    // Keep the fresh schema and its version marker in one SQLite transaction.
    // A process crash must never leave a partial set of tables that a later
    // open could accidentally bless as the current schema.
    let mut tx = pool.begin().await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS search_revisions (
            revision_id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            effective_hash TEXT NOT NULL,
            model_revision TEXT NOT NULL,
            embedding_revision TEXT,
            dimensions BIGINT CHECK (dimensions > 0),
            parser_version TEXT NOT NULL,
            chunker_version TEXT,
            ranking_version TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('building', 'ready', 'failed')),
            last_error TEXT,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            ready_at TEXT
        )",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS search_resources (
            revision_id TEXT NOT NULL REFERENCES search_revisions(revision_id) ON DELETE CASCADE,
            resource_id TEXT NOT NULL,
            project_id TEXT NOT NULL,
            scope TEXT NOT NULL CHECK (scope IN ('org', 'project')),
            kind TEXT NOT NULL CHECK (kind IN ('context', 'rule', 'workflow', 'memory')),
            path TEXT NOT NULL,
            title TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            content TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            source_commit_id TEXT,
            draft_id TEXT,
            draft_revision TEXT,
            PRIMARY KEY (revision_id, resource_id)
        )",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS search_units (
            unit_rowid INTEGER PRIMARY KEY AUTOINCREMENT,
            revision_id TEXT NOT NULL REFERENCES search_revisions(revision_id) ON DELETE CASCADE,
            unit_key TEXT NOT NULL,
            resource_id TEXT NOT NULL,
            ordinal BIGINT NOT NULL,
            heading_path_json TEXT NOT NULL,
            locator_json TEXT NOT NULL,
            text TEXT NOT NULL,
            text_hash TEXT NOT NULL,
            token_count BIGINT NOT NULL,
            vector BLOB NOT NULL,
            vector_input_hash TEXT,
            UNIQUE (revision_id, unit_key),
            FOREIGN KEY (revision_id, resource_id)
                REFERENCES search_resources(revision_id, resource_id) ON DELETE CASCADE
        )",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS search_vector_cache (
            vector_input_hash TEXT PRIMARY KEY,
            embedding_revision TEXT NOT NULL,
            input_version TEXT NOT NULL,
            dimensions BIGINT NOT NULL CHECK (dimensions > 0),
            vector BLOB NOT NULL,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            last_used_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_search_units_revision
         ON search_units (revision_id, unit_key)",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_search_units_vector_input_hash
         ON search_units (vector_input_hash)",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "CREATE VIRTUAL TABLE IF NOT EXISTS search_units_fts USING fts5(
            revision_id UNINDEXED,
            unit_key UNINDEXED,
            path,
            title,
            heading,
            body,
            tokenize = 'trigram case_sensitive 0'
        )",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS search_heads (
            project_id TEXT PRIMARY KEY,
            revision_id TEXT NOT NULL REFERENCES search_revisions(revision_id)
        )",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO search_meta (key, value) VALUES ('schema_version', $1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(PROJECT_INDEX_SCHEMA_VERSION.to_string())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn create_vector_cache(tx: &mut Transaction<'_, Sqlite>) -> Result<(), DaemonError> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS search_vector_cache (
            vector_input_hash TEXT PRIMARY KEY,
            embedding_revision TEXT NOT NULL,
            input_version TEXT NOT NULL,
            dimensions BIGINT NOT NULL CHECK (dimensions > 0),
            vector BLOB NOT NULL,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            last_used_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) async fn ready_index_revision(
    state: &super::DaemonState,
    pool: &SqlitePool,
    project_id: &str,
) -> Result<Option<String>, DaemonError> {
    if state.inner.search_models.status() != super::SearchModelRuntimeStatus::Ready {
        return Ok(None);
    }
    let models = state.inner.search_models.clone();
    let (model_revision, embedding_revision) = super::run_model_work(state, move || {
        Ok((models.revision()?, models.embedding_revision()?))
    })
    .await?;
    let dimensions = i64::try_from(state.inner.search_models.dimensions())
        .map_err(|_| SearchFailure::vector("embedding dimensions exceed SQLite integer range"))?;
    ready_index_revision_for_fingerprint(
        pool,
        project_id,
        &model_revision,
        &embedding_revision,
        dimensions,
    )
    .await
}

async fn ready_index_revision_for_fingerprint(
    pool: &SqlitePool,
    project_id: &str,
    model_revision: &str,
    embedding_revision: &str,
    dimensions: i64,
) -> Result<Option<String>, DaemonError> {
    Ok(sqlx::query_scalar(
        "SELECT h.revision_id
         FROM search_heads h
         JOIN search_revisions r ON r.revision_id = h.revision_id
         WHERE h.project_id = $1 AND r.status = 'ready'
           AND r.model_revision = $2
           AND r.embedding_revision = $3
           AND r.dimensions = $4
           AND r.parser_version = $5
           AND r.chunker_version = $6
           AND r.ranking_version = $7",
    )
    .bind(project_id)
    .bind(model_revision)
    .bind(embedding_revision)
    .bind(dimensions)
    .bind(PARSER_VERSION)
    .bind(CHUNKER_VERSION)
    .bind(RANKING_CONFIG_VERSION)
    .fetch_optional(pool)
    .await?)
}

pub(super) async fn prepare_incremental_index<F, Fut>(
    state: &super::DaemonState,
    pool: &SqlitePool,
    effective: &EffectiveMemory,
    mut should_continue: F,
) -> Result<PrepareIndexOutcome, DaemonError>
where
    F: FnMut() -> Fut + Send,
    Fut: Future<Output = bool> + Send,
{
    match state.inner.search_models.status() {
        super::SearchModelRuntimeStatus::Missing => {
            return Err(SearchFailure::model_preparing(
                "search models are waiting for background preparation",
            )
            .into());
        }
        super::SearchModelRuntimeStatus::Preparing {
            downloaded_bytes,
            total_bytes,
        } => {
            return Err(SearchFailure::model_preparing(format!(
                "search models are preparing ({downloaded_bytes}/{total_bytes} bytes)"
            ))
            .into());
        }
        super::SearchModelRuntimeStatus::Failed => {
            return Err(SearchFailure::model(
                "search model preparation failed and will retry in the background",
            )
            .into());
        }
        super::SearchModelRuntimeStatus::Ready => {}
    }
    if !should_continue().await {
        return Ok(PrepareIndexOutcome::Superseded);
    }
    let models = state.inner.search_models.clone();
    let (model_revision, embedding_revision) = super::run_model_work(state, move || {
        let model_revision = models.revision()?;
        let embedding_revision = models.embedding_revision()?;
        Ok((model_revision, embedding_revision))
    })
    .await?;
    let dimensions = state.inner.search_models.dimensions();
    let revision_id = index_revision_id(
        &effective.effective_hash,
        &model_revision,
        &embedding_revision,
        dimensions,
    );
    let existing: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM search_heads h
         JOIN search_revisions r ON r.revision_id = h.revision_id
         WHERE h.project_id = $1 AND h.revision_id = $2 AND r.status = 'ready'",
    )
    .bind(&effective.project_id)
    .bind(&revision_id)
    .fetch_one(pool)
    .await?;
    if existing == 1 {
        return Ok(PrepareIndexOutcome::AlreadyReady(revision_id));
    }

    let result: Result<PrepareIndexOutcome, DaemonError> = async {
        let mut built = Vec::new();
        for (resource_index, resource) in effective.resources.iter().enumerate() {
            if !should_continue().await {
                return Ok(PrepareIndexOutcome::Superseded);
            }
            if let Some(mut reused) = reuse_ready_resource(
                pool,
                &effective.project_id,
                resource_index,
                resource,
                &embedding_revision,
                dimensions,
            )
            .await?
            {
                built.append(&mut reused);
                continue;
            }

            let resource = resource.clone();
            let models = state.inner.search_models.clone();
            let units =
                super::run_model_work(state, move || build_units(&resource, models.as_ref()))
                    .await?;
            for unit_batch in units.chunks(INDEX_EMBED_BATCH_SIZE) {
                if !should_continue().await {
                    return Ok(PrepareIndexOutcome::Superseded);
                }
                let mut batch = build_incremental_batch(
                    state,
                    pool,
                    state.inner.search_models.clone(),
                    &effective.resources[resource_index],
                    resource_index,
                    unit_batch,
                    &embedding_revision,
                    dimensions,
                )
                .await?;
                built.append(&mut batch);
                if !should_continue().await {
                    return Ok(PrepareIndexOutcome::Superseded);
                }
            }
        }
        Ok(PrepareIndexOutcome::Prepared(PreparedIndex {
            project_id: effective.project_id.clone(),
            effective_hash: effective.effective_hash.clone(),
            revision_id: revision_id.clone(),
            model_revision: model_revision.clone(),
            embedding_revision: embedding_revision.clone(),
            dimensions,
            units: built,
        }))
    }
    .await;
    if let Err(error) = &result {
        let _ = record_failed_index(
            pool,
            effective,
            &revision_id,
            &model_revision,
            &embedding_revision,
            dimensions,
            &error.to_string(),
        )
        .await;
    }
    result
}

async fn record_failed_index(
    pool: &SqlitePool,
    effective: &EffectiveMemory,
    revision_id: &str,
    model_revision: &str,
    embedding_revision: &str,
    dimensions: usize,
    message: &str,
) -> Result<(), DaemonError> {
    sqlx::query(
        "INSERT INTO search_revisions (
            revision_id, project_id, effective_hash, model_revision, embedding_revision,
            dimensions, parser_version, chunker_version, ranking_version, status, last_error
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'failed', $10)
         ON CONFLICT(revision_id) DO UPDATE SET
            status = 'failed',
            last_error = excluded.last_error,
            ready_at = NULL",
    )
    .bind(revision_id)
    .bind(&effective.project_id)
    .bind(&effective.effective_hash)
    .bind(model_revision)
    .bind(embedding_revision)
    .bind(
        i64::try_from(dimensions).map_err(|_| {
            SearchFailure::vector("embedding dimensions exceed SQLite integer range")
        })?,
    )
    .bind(PARSER_VERSION)
    .bind(CHUNKER_VERSION)
    .bind(RANKING_CONFIG_VERSION)
    .bind(message)
    .execute(pool)
    .await?;
    Ok(())
}

async fn reuse_ready_resource(
    pool: &SqlitePool,
    project_id: &str,
    resource_index: usize,
    resource: &SourceResource,
    embedding_revision: &str,
    dimensions: usize,
) -> Result<Option<Vec<BuiltUnit>>, DaemonError> {
    let metadata = sqlx::query(
        "SELECT h.revision_id, r.embedding_revision, r.dimensions,
                r.parser_version, r.chunker_version,
                sr.content_hash, sr.path, sr.scope, sr.kind
         FROM search_heads h
         JOIN search_revisions r ON r.revision_id = h.revision_id
         JOIN search_resources sr ON sr.revision_id = h.revision_id
         WHERE h.project_id = $1 AND r.status = 'ready' AND sr.resource_id = $2",
    )
    .bind(project_id)
    .bind(&resource.resource_id)
    .fetch_optional(pool)
    .await?;
    let Some(metadata) = metadata else {
        return Ok(None);
    };
    if metadata
        .try_get::<Option<String>, _>("embedding_revision")?
        .as_deref()
        != Some(embedding_revision)
        || metadata.try_get::<Option<i64>, _>("dimensions")? != i64::try_from(dimensions).ok()
        || metadata.try_get::<String, _>("parser_version")? != PARSER_VERSION
        || metadata
            .try_get::<Option<String>, _>("chunker_version")?
            .as_deref()
            != Some(CHUNKER_VERSION)
        || metadata.try_get::<String, _>("content_hash")? != resource.content_hash
        || metadata.try_get::<String, _>("path")? != resource.path
        || metadata.try_get::<String, _>("scope")? != resource.scope.as_str()
        || metadata.try_get::<String, _>("kind")? != resource.kind.as_str()
    {
        return Ok(None);
    }

    let revision_id = metadata.try_get::<String, _>("revision_id")?;
    let rows = sqlx::query(
        "SELECT unit_key, resource_id, ordinal, heading_path_json, locator_json,
                text, text_hash, token_count, vector, vector_input_hash
         FROM search_units
         WHERE revision_id = $1 AND resource_id = $2
         ORDER BY ordinal",
    )
    .bind(revision_id)
    .bind(&resource.resource_id)
    .fetch_all(pool)
    .await?;
    let mut reused = Vec::with_capacity(rows.len());
    let mut reused_hashes = Vec::with_capacity(rows.len());
    for row in rows {
        let ordinal = row.try_get::<i64, _>("ordinal")?;
        let token_count = row.try_get::<i64, _>("token_count")?;
        let (Ok(ordinal), Ok(token_count)) =
            (usize::try_from(ordinal), usize::try_from(token_count))
        else {
            return Ok(None);
        };
        let Ok(heading_path) =
            serde_json::from_str::<Vec<String>>(&row.try_get::<String, _>("heading_path_json")?)
        else {
            return Ok(None);
        };
        let Ok(locator) = serde_json::from_str::<super::SourceLocator>(
            &row.try_get::<String, _>("locator_json")?,
        ) else {
            return Ok(None);
        };
        let unit = RetrievalUnit {
            unit_key: row.try_get("unit_key")?,
            resource_id: row.try_get("resource_id")?,
            ordinal,
            heading_path,
            locator,
            text: row.try_get("text")?,
            text_hash: row.try_get("text_hash")?,
            token_count,
        };
        if unit.resource_id != resource.resource_id {
            return Ok(None);
        }
        let expected_hash = vector_input_hash(
            embedding_revision,
            dimensions,
            &passage_input(resource, &unit),
        );
        let Some(stored_hash) = row.try_get::<Option<String>, _>("vector_input_hash")? else {
            return Ok(None);
        };
        if stored_hash != expected_hash {
            return Ok(None);
        }
        let Ok(vector) = decode_vector(&row.try_get::<Vec<u8>, _>("vector")?, dimensions) else {
            return Ok(None);
        };
        reused_hashes.push(expected_hash.clone());
        reused.push(BuiltUnit {
            resource_index,
            unit,
            vector,
            vector_input_hash: expected_hash,
        });
    }
    touch_cached_vectors(pool, &reused_hashes).await?;
    Ok(Some(reused))
}

#[allow(clippy::too_many_arguments)]
async fn build_incremental_batch(
    state: &super::DaemonState,
    pool: &SqlitePool,
    models: std::sync::Arc<dyn SearchModels>,
    resource: &SourceResource,
    resource_index: usize,
    units: &[RetrievalUnit],
    embedding_revision: &str,
    dimensions: usize,
) -> Result<Vec<BuiltUnit>, DaemonError> {
    let inputs = units
        .iter()
        .cloned()
        .map(|unit| {
            let passage = passage_input(resource, &unit);
            let hash = vector_input_hash(embedding_revision, dimensions, &passage);
            (unit, passage, hash)
        })
        .collect::<Vec<_>>();
    let hashes = inputs
        .iter()
        .map(|(_, _, hash)| hash.clone())
        .collect::<Vec<_>>();
    let cached = load_cached_vectors(pool, &hashes, embedding_revision, dimensions).await?;
    let mut vectors = vec![None; inputs.len()];
    let mut missing_indexes = Vec::new();
    let mut missing_passages = Vec::new();
    for (index, (_, passage, hash)) in inputs.iter().enumerate() {
        if let Some(vector) = cached.get(hash) {
            vectors[index] = Some(vector.clone());
        } else {
            missing_indexes.push(index);
            missing_passages.push(passage.clone());
        }
    }
    if !missing_passages.is_empty() {
        let embedded =
            super::run_model_work(state, move || models.embed_passages(&missing_passages)).await?;
        if embedded.len() != missing_indexes.len() {
            return Err(SearchFailure::vector(format!(
                "embedding count {} does not match cache miss count {}",
                embedded.len(),
                missing_indexes.len()
            ))
            .into());
        }
        let mut writes = Vec::with_capacity(embedded.len());
        for (index, vector) in missing_indexes.into_iter().zip(embedded) {
            if vector.len() != dimensions || !valid_normalized_vector(&vector) {
                return Err(SearchFailure::vector(
                    "incremental index build produced a corrupt or non-normalized vector",
                )
                .into());
            }
            writes.push((inputs[index].2.clone(), vector.clone()));
            vectors[index] = Some(vector);
        }
        persist_cached_vectors(pool, embedding_revision, dimensions, &writes).await?;
    }
    touch_cached_vectors(pool, &hashes).await?;

    inputs
        .into_iter()
        .zip(vectors)
        .map(|((unit, _, vector_input_hash), vector)| {
            Ok(BuiltUnit {
                resource_index,
                unit,
                vector: vector.ok_or_else(|| {
                    SearchFailure::vector("incremental vector cache returned an incomplete batch")
                })?,
                vector_input_hash,
            })
        })
        .collect::<Result<Vec<_>, SearchFailure>>()
        .map_err(DaemonError::from)
}

async fn load_cached_vectors(
    pool: &SqlitePool,
    hashes: &[String],
    embedding_revision: &str,
    dimensions: usize,
) -> Result<HashMap<String, Vec<f32>>, DaemonError> {
    if hashes.is_empty() {
        return Ok(HashMap::new());
    }
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT vector_input_hash, embedding_revision, input_version, dimensions, vector
         FROM search_vector_cache WHERE vector_input_hash IN (",
    );
    for (index, hash) in hashes.iter().enumerate() {
        if index != 0 {
            query.push(", ");
        }
        query.push_bind(hash);
    }
    query.push(")");
    let rows = query.build().fetch_all(pool).await?;
    let mut cached = HashMap::with_capacity(rows.len());
    for row in rows {
        if row.try_get::<String, _>("embedding_revision")? != embedding_revision
            || row.try_get::<String, _>("input_version")? != VECTOR_INPUT_VERSION
            || row.try_get::<i64, _>("dimensions")? != dimensions as i64
        {
            continue;
        }
        let hash = row.try_get::<String, _>("vector_input_hash")?;
        if let Ok(vector) = decode_vector(&row.try_get::<Vec<u8>, _>("vector")?, dimensions) {
            cached.insert(hash, vector);
        }
    }
    Ok(cached)
}

async fn persist_cached_vectors(
    pool: &SqlitePool,
    embedding_revision: &str,
    dimensions: usize,
    vectors: &[(String, Vec<f32>)],
) -> Result<(), DaemonError> {
    let mut tx = pool.begin().await?;
    for (hash, vector) in vectors {
        sqlx::query(
            "INSERT INTO search_vector_cache (
                vector_input_hash, embedding_revision, input_version, dimensions, vector
             ) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT(vector_input_hash) DO UPDATE SET
                embedding_revision = excluded.embedding_revision,
                input_version = excluded.input_version,
                dimensions = excluded.dimensions,
                vector = excluded.vector,
                last_used_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        )
        .bind(hash)
        .bind(embedding_revision)
        .bind(VECTOR_INPUT_VERSION)
        .bind(dimensions as i64)
        .bind(encode_vector(vector))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn touch_cached_vectors(pool: &SqlitePool, hashes: &[String]) -> Result<(), DaemonError> {
    if hashes.is_empty() {
        return Ok(());
    }
    let mut query = QueryBuilder::<Sqlite>::new(
        "UPDATE search_vector_cache
         SET last_used_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE vector_input_hash IN (",
    );
    for (index, hash) in hashes.iter().enumerate() {
        if index != 0 {
            query.push(", ");
        }
        query.push_bind(hash);
    }
    query.push(")");
    query.build().execute(pool).await?;
    Ok(())
}

fn passage_input(resource: &SourceResource, unit: &RetrievalUnit) -> String {
    format!(
        "{}\n{}\n{}",
        resource.path,
        unit.heading_path.join(" > "),
        unit.text
    )
}

pub(super) fn vector_input_hash(
    embedding_revision: &str,
    dimensions: usize,
    passage: &str,
) -> String {
    let mut hasher = Sha256::new();
    for value in [
        VECTOR_INPUT_VERSION,
        embedding_revision,
        &dimensions.to_string(),
        passage,
    ] {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    format!("vector_{}", hex::encode(hasher.finalize()))
}

#[cfg(test)]
pub(super) fn build_index_units(
    resources: &[SourceResource],
    models: &dyn SearchModels,
) -> Result<Vec<BuiltUnit>, SearchFailure> {
    let embedding_revision = models.embedding_revision()?;
    let dimensions = models.dimensions();
    let mut built = Vec::new();
    let mut pending = Vec::with_capacity(INDEX_EMBED_BATCH_SIZE);
    for (resource_index, resource) in resources.iter().enumerate() {
        for unit in build_units(resource, models)? {
            pending.push((resource_index, unit));
            if pending.len() == INDEX_EMBED_BATCH_SIZE {
                embed_pending_units(
                    resources,
                    models,
                    &embedding_revision,
                    dimensions,
                    &mut pending,
                    &mut built,
                )?;
            }
        }
    }
    embed_pending_units(
        resources,
        models,
        &embedding_revision,
        dimensions,
        &mut pending,
        &mut built,
    )?;
    Ok(built)
}

#[cfg(test)]
fn embed_pending_units(
    resources: &[SourceResource],
    models: &dyn SearchModels,
    embedding_revision: &str,
    dimensions: usize,
    pending: &mut Vec<(usize, RetrievalUnit)>,
    built: &mut Vec<BuiltUnit>,
) -> Result<(), SearchFailure> {
    if pending.is_empty() {
        return Ok(());
    }
    let passages = pending
        .iter()
        .map(|(resource_index, unit)| {
            let resource = &resources[*resource_index];
            passage_input(resource, unit)
        })
        .collect::<Vec<_>>();
    let embeddings = models.embed_passages(&passages)?;
    if embeddings.len() != pending.len() {
        return Err(SearchFailure::vector(format!(
            "embedding count {} does not match unit count {}",
            embeddings.len(),
            pending.len()
        )));
    }
    built.extend(pending.drain(..).zip(passages).zip(embeddings).map(
        |(((resource_index, unit), passage), vector)| BuiltUnit {
            resource_index,
            unit,
            vector,
            vector_input_hash: vector_input_hash(embedding_revision, dimensions, &passage),
        },
    ));
    Ok(())
}

pub(super) fn index_revision_id(
    effective_hash: &str,
    model_revision: &str,
    embedding_revision: &str,
    dimensions: usize,
) -> String {
    let mut hasher = Sha256::new();
    for value in [
        effective_hash,
        PARSER_VERSION,
        CHUNKER_VERSION,
        model_revision,
        embedding_revision,
        VECTOR_INPUT_VERSION,
        RANKING_CONFIG_VERSION,
    ] {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    hasher.update(dimensions.to_le_bytes());
    format!("search_{}", hex::encode(hasher.finalize()))
}

pub(super) async fn stage_prepared_index(
    pool: &SqlitePool,
    effective: &EffectiveMemory,
    prepared: &PreparedIndex,
) -> Result<String, DaemonError> {
    write_prepared_index(pool, effective, prepared).await
}

async fn write_prepared_index(
    pool: &SqlitePool,
    effective: &EffectiveMemory,
    prepared: &PreparedIndex,
) -> Result<String, DaemonError> {
    if prepared.project_id != effective.project_id
        || prepared.effective_hash != effective.effective_hash
        || prepared.revision_id
            != index_revision_id(
                &prepared.effective_hash,
                &prepared.model_revision,
                &prepared.embedding_revision,
                prepared.dimensions,
            )
    {
        return Err(SearchFailure::generation_changed(
            "prepared search index no longer matches the requested Effective Memory",
        )
        .into());
    }
    if prepared.units.iter().any(|built| {
        built.vector.len() != prepared.dimensions
            || !valid_normalized_vector(&built.vector)
            || effective
                .resources
                .get(built.resource_index)
                .is_none_or(|resource| {
                    resource.resource_id != built.unit.resource_id
                        || built.vector_input_hash
                            != vector_input_hash(
                                &prepared.embedding_revision,
                                prepared.dimensions,
                                &passage_input(resource, &built.unit),
                            )
                })
    }) {
        return Err(SearchFailure::vector(
            "index build produced a corrupt or non-normalized vector",
        )
        .into());
    }
    let mut tx = pool.begin().await?;
    let already_ready: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM search_revisions
         WHERE project_id = $1 AND revision_id = $2 AND status = 'ready'",
    )
    .bind(&effective.project_id)
    .bind(&prepared.revision_id)
    .fetch_one(&mut *tx)
    .await?;
    if already_ready == 1 {
        tx.commit().await?;
        return Ok(prepared.revision_id.clone());
    }
    delete_revision(&mut tx, &prepared.revision_id).await?;
    sqlx::query(
        "INSERT INTO search_revisions (
            revision_id, project_id, effective_hash, model_revision, embedding_revision,
            dimensions, parser_version, chunker_version, ranking_version, status
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'building')",
    )
    .bind(&prepared.revision_id)
    .bind(&effective.project_id)
    .bind(&effective.effective_hash)
    .bind(&prepared.model_revision)
    .bind(&prepared.embedding_revision)
    .bind(
        i64::try_from(prepared.dimensions).map_err(|_| {
            SearchFailure::vector("embedding dimensions exceed SQLite integer range")
        })?,
    )
    .bind(PARSER_VERSION)
    .bind(CHUNKER_VERSION)
    .bind(RANKING_CONFIG_VERSION)
    .execute(&mut *tx)
    .await?;

    for resource in effective.resources.iter() {
        sqlx::query(
            "INSERT INTO search_resources (
                revision_id, resource_id, project_id, scope, kind, path, title,
                description, content, content_hash, source_commit_id, draft_id, draft_revision
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        )
        .bind(&prepared.revision_id)
        .bind(&resource.resource_id)
        .bind(&resource.project_id)
        .bind(resource.scope.as_str())
        .bind(resource.kind.as_str())
        .bind(&resource.path)
        .bind(&resource.title)
        .bind(&resource.description)
        .bind(&resource.content)
        .bind(&resource.content_hash)
        .bind(&resource.source_commit_id)
        .bind(&resource.draft_id)
        .bind(&resource.draft_revision)
        .execute(&mut *tx)
        .await?;
    }

    for built in &prepared.units {
        let resource = &effective.resources[built.resource_index];
        let result = sqlx::query(
            "INSERT INTO search_units (
                revision_id, unit_key, resource_id, ordinal, heading_path_json,
                locator_json, text, text_hash, token_count, vector, vector_input_hash
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(&prepared.revision_id)
        .bind(&built.unit.unit_key)
        .bind(&built.unit.resource_id)
        .bind(built.unit.ordinal as i64)
        .bind(serde_json::to_string(&built.unit.heading_path)?)
        .bind(serde_json::to_string(&built.unit.locator)?)
        .bind(&built.unit.text)
        .bind(&built.unit.text_hash)
        .bind(built.unit.token_count as i64)
        .bind(encode_vector(&built.vector))
        .bind(&built.vector_input_hash)
        .execute(&mut *tx)
        .await?;
        let rowid = result.last_insert_rowid();
        sqlx::query(
            "INSERT INTO search_units_fts (
                rowid, revision_id, unit_key, path, title, heading, body
             ) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(rowid)
        .bind(&prepared.revision_id)
        .bind(&built.unit.unit_key)
        .bind(&resource.path)
        .bind(&resource.title)
        .bind(built.unit.heading_path.join(" > "))
        .bind(&built.unit.text)
        .execute(&mut *tx)
        .await?;
    }

    let resource_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM search_resources WHERE revision_id = $1")
            .bind(&prepared.revision_id)
            .fetch_one(&mut *tx)
            .await?;
    let unit_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM search_units WHERE revision_id = $1")
            .bind(&prepared.revision_id)
            .fetch_one(&mut *tx)
            .await?;
    if resource_count != effective.resources.len() as i64
        || unit_count != prepared.units.len() as i64
    {
        return Err(SearchFailure::failed(
            "search index row counts do not match the Effective Memory build",
        )
        .into());
    }

    sqlx::query(
        "UPDATE search_revisions
         SET status = 'ready', ready_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), last_error = NULL
         WHERE revision_id = $1",
    )
    .bind(&prepared.revision_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(prepared.revision_id.clone())
}

pub(super) async fn publish_staged_index(
    pool: &SqlitePool,
    project_id: &str,
    effective_hash: &str,
    revision_id: &str,
) -> Result<(), DaemonError> {
    let mut tx = pool.begin().await?;
    let ready: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM search_revisions
         WHERE project_id = $1 AND effective_hash = $2
           AND revision_id = $3 AND status = 'ready'",
    )
    .bind(project_id)
    .bind(effective_hash)
    .bind(revision_id)
    .fetch_one(&mut *tx)
    .await?;
    if ready != 1 {
        return Err(SearchFailure::generation_changed(
            "staged search index is not ready for publication",
        )
        .into());
    }
    publish_revision_head_in_tx(&mut tx, project_id, revision_id).await?;
    tx.commit().await?;
    Ok(())
}

pub(super) async fn prune_old_ready_revisions(
    pool: &SqlitePool,
    project_id: &str,
    retain: usize,
) -> Result<(), DaemonError> {
    let retain = retain.max(1) as i64;
    let mut tx = pool.begin().await?;
    let obsolete = sqlx::query_scalar::<_, String>(
        "SELECT revision_id FROM search_revisions
         WHERE project_id = $1 AND status = 'ready'
           AND revision_id NOT IN (
               SELECT revision_id FROM search_revisions
               WHERE project_id = $1 AND status = 'ready'
               ORDER BY ready_at DESC, created_at DESC, revision_id DESC
               LIMIT $2
           )
           AND revision_id NOT IN (
               SELECT revision_id FROM search_heads WHERE project_id = $1
           )",
    )
    .bind(project_id)
    .bind(retain)
    .fetch_all(&mut *tx)
    .await?;
    for revision_id in obsolete {
        delete_revision(&mut tx, &revision_id).await?;
    }
    prune_unused_vector_cache(&mut tx, VECTOR_CACHE_RETAIN_UNUSED).await?;
    tx.commit().await?;
    Ok(())
}

async fn prune_unused_vector_cache(
    tx: &mut Transaction<'_, Sqlite>,
    retain_unused: i64,
) -> Result<(), DaemonError> {
    let unused_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM search_vector_cache c
         WHERE NOT EXISTS (
             SELECT 1 FROM search_units u
             WHERE u.vector_input_hash = c.vector_input_hash
         )",
    )
    .fetch_one(&mut **tx)
    .await?;
    let delete_count = unused_count.saturating_sub(retain_unused.max(0));
    if delete_count == 0 {
        return Ok(());
    }
    sqlx::query(
        "DELETE FROM search_vector_cache
         WHERE vector_input_hash IN (
             SELECT c.vector_input_hash FROM search_vector_cache c
             WHERE NOT EXISTS (
                 SELECT 1 FROM search_units u
                 WHERE u.vector_input_hash = c.vector_input_hash
             )
             ORDER BY c.last_used_at ASC, c.vector_input_hash ASC
             LIMIT $1
         )",
    )
    .bind(delete_count)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn publish_revision_head_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    revision_id: &str,
) -> Result<(), DaemonError> {
    sqlx::query(
        "INSERT INTO search_heads (project_id, revision_id)
         VALUES ($1, $2)
         ON CONFLICT(project_id) DO UPDATE SET revision_id = excluded.revision_id",
    )
    .bind(project_id)
    .bind(revision_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) async fn delete_revision(
    tx: &mut Transaction<'_, Sqlite>,
    revision_id: &str,
) -> Result<(), DaemonError> {
    sqlx::query(
        "DELETE FROM search_units_fts
         WHERE rowid IN (SELECT unit_rowid FROM search_units WHERE revision_id = $1)",
    )
    .bind(revision_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM search_revisions WHERE revision_id = $1")
        .bind(revision_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn encode_vector(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(vector));
    for value in vector {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

pub(super) fn decode_vector(bytes: &[u8], dimensions: usize) -> Result<Vec<f32>, SearchFailure> {
    if bytes.len() != dimensions * std::mem::size_of::<f32>() {
        return Err(SearchFailure::vector(format!(
            "vector byte length {} does not match dimension {dimensions}",
            bytes.len()
        )));
    }
    let vector = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect::<Vec<_>>();
    if !valid_normalized_vector(&vector) {
        return Err(SearchFailure::vector(
            "stored vector contains invalid values or is not normalized",
        ));
    }
    Ok(vector)
}

pub(super) fn valid_normalized_vector(vector: &[f32]) -> bool {
    if vector.is_empty() || vector.iter().any(|value| !value.is_finite()) {
        return false;
    }
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    norm.is_finite() && (norm - 1.0).abs() <= 0.01
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::super::{MemoryKind, SourceScope};
    use super::*;
    use crate::{CredentialStore, CredentialStoreError, DaemonState, ServerCredentials};

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

    #[derive(Default)]
    struct RecordingModels {
        embedded: AtomicUsize,
        passages: Mutex<Vec<String>>,
    }

    impl RecordingModels {
        fn reset(&self) {
            self.embedded.store(0, Ordering::Relaxed);
            self.passages.lock().unwrap().clear();
        }

        fn embedded(&self) -> usize {
            self.embedded.load(Ordering::Relaxed)
        }

        fn passages(&self) -> Vec<String> {
            self.passages.lock().unwrap().clone()
        }
    }

    impl SearchModels for RecordingModels {
        fn revision(&self) -> Result<String, SearchFailure> {
            Ok("acceptance-models.v1".to_owned())
        }

        fn token_offsets(&self, text: &str) -> Result<Vec<(usize, usize)>, SearchFailure> {
            Ok(text
                .char_indices()
                .map(|(start, character)| (start, start + character.len_utf8()))
                .collect())
        }

        fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, SearchFailure> {
            self.embedded.fetch_add(texts.len(), Ordering::Relaxed);
            self.passages.lock().unwrap().extend_from_slice(texts);
            Ok(vec![vec![1.0, 0.0, 0.0]; texts.len()])
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

        fn status(&self) -> super::super::SearchModelRuntimeStatus {
            super::super::SearchModelRuntimeStatus::Ready
        }
    }

    async fn memory_pool() -> SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    async fn test_state(root: &Path, search_models: Arc<dyn SearchModels>) -> DaemonState {
        let mut config = crate::DaemonConfig::for_root(root.join("daemon"));
        config.project.server_url = "https://clumsies.test".to_owned();
        config.project.project_id = Some("prj_test".to_owned());
        DaemonState::initialize_with_credential_store_and_search_models(
            config,
            Arc::new(NoCredentials),
            search_models,
        )
        .await
        .unwrap()
    }

    fn resource(id: &str, path: &str, title: &str, content: String) -> SourceResource {
        SourceResource {
            resource_id: id.to_owned(),
            project_id: "prj_test".to_owned(),
            scope: SourceScope::Project,
            kind: MemoryKind::Memory,
            path: path.to_owned(),
            title: title.to_owned(),
            description: String::new(),
            content_hash: super::super::sha256(&content),
            content,
            source_commit_id: Some("commit_one".to_owned()),
            draft_id: None,
            draft_revision: None,
        }
    }

    fn remap_resource_ids(
        source: &EffectiveMemory,
        effective_hash: &str,
        suffix: &str,
    ) -> EffectiveMemory {
        let mut resources = source.resources.to_vec();
        for resource in &mut resources {
            resource.resource_id = format!("{}_{}", resource.resource_id, suffix);
        }
        EffectiveMemory {
            project_id: source.project_id.clone(),
            effective_hash: effective_hash.to_owned(),
            resources: resources.into(),
        }
    }

    async fn prepared(
        state: &DaemonState,
        pool: &SqlitePool,
        effective: &EffectiveMemory,
    ) -> PreparedIndex {
        match prepare_incremental_index(state, pool, effective, || async { true })
            .await
            .unwrap()
        {
            PrepareIndexOutcome::Prepared(prepared) => prepared,
            other => panic!("expected prepared index, got {other:?}"),
        }
    }

    async fn publish(
        pool: &SqlitePool,
        effective: &EffectiveMemory,
        prepared: &PreparedIndex,
    ) -> String {
        let revision = stage_prepared_index(pool, effective, prepared)
            .await
            .unwrap();
        publish_staged_index(
            pool,
            &effective.project_id,
            &effective.effective_hash,
            &revision,
        )
        .await
        .unwrap();
        revision
    }

    async fn connect_concurrently(path: &Path, count: usize) -> Vec<SqlitePool> {
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..count {
            let path = path.to_owned();
            tasks.spawn(async move { connect_project_index(&path).await });
        }
        let mut pools = Vec::with_capacity(count);
        while let Some(result) = tasks.join_next().await {
            pools.push(result.unwrap().unwrap());
        }
        pools
    }

    async fn seed_v5_file(path: &Path) {
        let pool = connect_project_index(path).await.unwrap();
        sqlx::query("DROP INDEX IF EXISTS idx_search_units_vector_input_hash")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DROP TABLE search_vector_cache")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("ALTER TABLE search_revisions DROP COLUMN dimensions")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE search_meta SET value = '5' WHERE key = 'schema_version'")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }

    async fn seed_early_v6_file(path: &Path) {
        let pool = connect_project_index(path).await.unwrap();
        sqlx::query("DROP INDEX idx_search_units_vector_input_hash")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }

    async fn assert_current_schema(pool: &SqlitePool) {
        let version: String =
            sqlx::query_scalar("SELECT value FROM search_meta WHERE key = 'schema_version'")
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(version, PROJECT_INDEX_SCHEMA_VERSION.to_string());
        assert!(has_column(pool, "search_revisions", "dimensions").await);
        let cache_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name = 'search_vector_cache'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(cache_count, 1);
        let index_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'index' AND name = 'idx_search_units_vector_input_hash'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(index_count, 1);
    }

    async fn has_column(pool: &SqlitePool, table: &str, column: &str) -> bool {
        let query = format!("PRAGMA table_info({table})");
        sqlx::query(&query)
            .fetch_all(pool)
            .await
            .unwrap()
            .iter()
            .any(|row| row.get::<String, _>("name") == column)
    }

    #[tokio::test]
    async fn file_index_reuses_only_valid_persistent_vectors() {
        let root = tempfile::tempdir().unwrap();
        let models = Arc::new(RecordingModels::default());
        let state = test_state(root.path(), models.clone()).await;
        let index_path = root.path().join("project-index").join("search.sqlite");
        let pool = connect_project_index(&index_path).await.unwrap();
        let alpha = format!(
            "# Alpha\n\n{}\n\n# Beta\n\n{}\n",
            "a".repeat(900),
            "b".repeat(900)
        );
        let gamma = format!("# Gamma\n\n{}\n", "c".repeat(900));
        let cold = EffectiveMemory {
            project_id: "prj_test".to_owned(),
            effective_hash: "effective_cold".to_owned(),
            resources: vec![
                resource("ctx_alpha", "context/alpha.md", "Alpha", alpha.clone()),
                resource("ctx_gamma", "context/gamma.md", "Gamma", gamma),
            ]
            .into(),
        };

        let cold_prepared = prepared(&state, &pool, &cold).await;
        let cold_embedded = models.embedded();
        assert!(
            cold_embedded >= 3,
            "fixture must produce multiple cache rows"
        );
        publish(&pool, &cold, &cold_prepared).await;

        models.reset();
        assert!(matches!(
            prepare_incremental_index(&state, &pool, &cold, || async { true })
                .await
                .unwrap(),
            PrepareIndexOutcome::AlreadyReady(_)
        ));
        assert_eq!(models.embedded(), 0, "the ready head must not re-embed");

        let mut changed_resources = cold.resources.to_vec();
        let body_start = changed_resources[0].content.find("\n\n").unwrap() + 2;
        changed_resources[0]
            .content
            .replace_range(body_start..body_start + 1, "z");
        changed_resources[0].content_hash = super::super::sha256(&changed_resources[0].content);
        let changed = EffectiveMemory {
            project_id: "prj_test".to_owned(),
            effective_hash: "effective_changed".to_owned(),
            resources: changed_resources.into(),
        };
        models.reset();
        let changed_prepared = prepared(&state, &pool, &changed).await;
        assert_eq!(models.embedded(), 1, "one changed chunk must be one miss");
        assert_eq!(models.passages().len(), 1);
        publish(&pool, &changed, &changed_prepared).await;

        pool.close().await;
        let pool = connect_project_index(&index_path).await.unwrap();
        let reopen = remap_resource_ids(&changed, "effective_reopen", "reopen");
        models.reset();
        let reopen_prepared = prepared(&state, &pool, &reopen).await;
        assert_eq!(
            models.embedded(),
            0,
            "renamed resources must reuse vectors from the reopened file cache"
        );
        publish(&pool, &reopen, &reopen_prepared).await;

        let corrupt_hash: String = sqlx::query_scalar(
            "SELECT vector_input_hash FROM search_units
             WHERE revision_id = $1 ORDER BY unit_rowid LIMIT 1",
        )
        .bind(&reopen_prepared.revision_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE search_vector_cache SET vector = x'0001'
             WHERE vector_input_hash = $1",
        )
        .bind(&corrupt_hash)
        .execute(&pool)
        .await
        .unwrap();
        let repaired = remap_resource_ids(&reopen, "effective_repaired", "repaired");
        models.reset();
        let _ = prepared(&state, &pool, &repaired).await;
        assert_eq!(
            models.embedded(),
            1,
            "one corrupt cache row must repair only that vector"
        );
        assert_eq!(models.passages().len(), 1);
        let repaired_bytes: Vec<u8> = sqlx::query_scalar(
            "SELECT vector FROM search_vector_cache WHERE vector_input_hash = $1",
        )
        .bind(&corrupt_hash)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(decode_vector(&repaired_bytes, 3).is_ok());
        pool.close().await;
    }

    #[tokio::test]
    async fn concurrent_file_opens_idempotently_migrate_v5_and_repair_v6() {
        let root = tempfile::tempdir().unwrap();
        for (name, seed_v5) in [("v5.sqlite", true), ("v6.sqlite", false)] {
            let path = root.path().join(name);
            if seed_v5 {
                seed_v5_file(&path).await;
            } else {
                seed_early_v6_file(&path).await;
            }
            let pools = connect_concurrently(&path, 8).await;
            assert_eq!(pools.len(), 8);
            for pool in &pools {
                assert_current_schema(pool).await;
            }
            for pool in pools {
                pool.close().await;
            }
            let reopened = connect_project_index(&path).await.unwrap();
            assert_current_schema(&reopened).await;
            reopened.close().await;
        }
    }

    #[tokio::test]
    async fn project_index_v3_migration_preserves_ready_head() {
        let pool = memory_pool().await;
        for statement in [
            "CREATE TABLE search_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
            "INSERT INTO search_meta (key, value) VALUES ('schema_version', '3')",
            "CREATE TABLE search_revisions (
                revision_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                effective_hash TEXT NOT NULL,
                model_revision TEXT NOT NULL,
                parser_version TEXT NOT NULL,
                ranking_version TEXT NOT NULL,
                status TEXT NOT NULL CHECK (status IN ('building', 'ready', 'failed')),
                last_error TEXT,
                created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                ready_at TEXT
            )",
            "CREATE TABLE search_resources (
                revision_id TEXT NOT NULL REFERENCES search_revisions(revision_id) ON DELETE CASCADE,
                resource_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                scope TEXT NOT NULL,
                kind TEXT NOT NULL,
                path TEXT NOT NULL,
                title TEXT NOT NULL,
                content TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                source_commit_id TEXT,
                draft_id TEXT,
                draft_revision TEXT,
                PRIMARY KEY (revision_id, resource_id)
            )",
            "CREATE TABLE search_units (
                unit_rowid INTEGER PRIMARY KEY AUTOINCREMENT,
                revision_id TEXT NOT NULL REFERENCES search_revisions(revision_id) ON DELETE CASCADE,
                unit_key TEXT NOT NULL,
                resource_id TEXT NOT NULL,
                ordinal BIGINT NOT NULL,
                heading_path_json TEXT NOT NULL,
                locator_json TEXT NOT NULL,
                text TEXT NOT NULL,
                text_hash TEXT NOT NULL,
                token_count BIGINT NOT NULL,
                vector BLOB NOT NULL,
                UNIQUE (revision_id, unit_key),
                FOREIGN KEY (revision_id, resource_id)
                    REFERENCES search_resources(revision_id, resource_id) ON DELETE CASCADE
            )",
            "CREATE TABLE search_heads (
                project_id TEXT PRIMARY KEY,
                revision_id TEXT NOT NULL REFERENCES search_revisions(revision_id)
            )",
            "INSERT INTO search_revisions (
                revision_id, project_id, effective_hash, model_revision,
                parser_version, ranking_version, status
             ) VALUES ('search_ready', 'prj_test', 'effective', 'models',
                       'markdown-units.v1', 'agent_activation.v2', 'ready')",
            "INSERT INTO search_heads (project_id, revision_id)
             VALUES ('prj_test', 'search_ready')",
        ] {
            sqlx::query(statement).execute(&pool).await.unwrap();
        }

        migrate_project_index(&pool).await.unwrap();

        let version: String =
            sqlx::query_scalar("SELECT value FROM search_meta WHERE key = 'schema_version'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(version, PROJECT_INDEX_SCHEMA_VERSION.to_string());
        assert!(has_column(&pool, "search_revisions", "embedding_revision").await);
        assert!(has_column(&pool, "search_revisions", "dimensions").await);
        assert!(has_column(&pool, "search_units", "vector_input_hash").await);
        let preserved_head: String = sqlx::query_scalar(
            "SELECT revision_id FROM search_heads WHERE project_id = 'prj_test'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(preserved_head, "search_ready");
        let cache_exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name = 'search_vector_cache'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(cache_exists, 1);
    }

    #[tokio::test]
    async fn ready_head_requires_the_current_query_fingerprint() {
        let pool = memory_pool().await;
        migrate_project_index(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO search_revisions (
                revision_id, project_id, effective_hash, model_revision,
                embedding_revision, dimensions, parser_version,
                chunker_version, ranking_version, status
             ) VALUES ('search_ready', 'prj_test', 'effective', 'models.v1',
                       'embedding.v1', 3, $1, $2, $3, 'ready')",
        )
        .bind(PARSER_VERSION)
        .bind(CHUNKER_VERSION)
        .bind(RANKING_CONFIG_VERSION)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO search_heads (project_id, revision_id)
             VALUES ('prj_test', 'search_ready')",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            ready_index_revision_for_fingerprint(
                &pool,
                "prj_test",
                "models.v1",
                "embedding.v1",
                3,
            )
            .await
            .unwrap()
            .as_deref(),
            Some("search_ready")
        );
        assert!(
            ready_index_revision_for_fingerprint(
                &pool,
                "prj_test",
                "models.v2",
                "embedding.v2",
                3,
            )
            .await
            .unwrap()
            .is_none()
        );
        assert!(
            ready_index_revision_for_fingerprint(
                &pool,
                "prj_test",
                "models.v1",
                "embedding.v1",
                4,
            )
            .await
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn index_revision_identity_includes_embedding_dimensions() {
        let three = index_revision_id("effective", "model", "embedding", 3);
        let four = index_revision_id("effective", "model", "embedding", 4);
        assert_ne!(three, four);
    }

    #[tokio::test]
    async fn unknown_project_index_schema_is_rebuilt() {
        let pool = memory_pool().await;
        sqlx::query("CREATE TABLE search_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO search_meta (key, value) VALUES ('schema_version', '99')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE search_heads (project_id TEXT PRIMARY KEY, revision_id TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO search_heads VALUES ('prj_old', 'search_old')")
            .execute(&pool)
            .await
            .unwrap();

        migrate_project_index(&pool).await.unwrap();

        let heads: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM search_heads")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(heads, 0);
    }

    #[tokio::test]
    async fn partial_project_index_without_version_is_rebuilt_instead_of_blessed() {
        let pool = memory_pool().await;
        sqlx::query("CREATE TABLE search_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE search_revisions (
                revision_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                status TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE search_units (
                unit_rowid INTEGER PRIMARY KEY,
                revision_id TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        migrate_project_index(&pool).await.unwrap();

        let version: String =
            sqlx::query_scalar("SELECT value FROM search_meta WHERE key = 'schema_version'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(version, PROJECT_INDEX_SCHEMA_VERSION.to_string());
        assert!(has_column(&pool, "search_revisions", "embedding_revision").await);
        assert!(has_column(&pool, "search_revisions", "chunker_version").await);
        assert!(has_column(&pool, "search_revisions", "dimensions").await);
        assert!(has_column(&pool, "search_units", "vector_input_hash").await);
    }

    #[tokio::test]
    async fn current_schema_adds_the_vector_cache_gc_lookup_index() {
        let pool = memory_pool().await;
        migrate_project_index(&pool).await.unwrap();
        sqlx::query("DROP INDEX idx_search_units_vector_input_hash")
            .execute(&pool)
            .await
            .unwrap();

        // Simulate reopening an existing v6 database created before this
        // additive index existed. The idempotent current-schema path must
        // repair it without rebuilding the project index or bumping schema.
        migrate_project_index(&pool).await.unwrap();

        let version: String =
            sqlx::query_scalar("SELECT value FROM search_meta WHERE key = 'schema_version'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(version, PROJECT_INDEX_SCHEMA_VERSION.to_string());
        let plans = [
            "EXPLAIN QUERY PLAN
             SELECT COUNT(*) FROM search_vector_cache c
             WHERE NOT EXISTS (
                 SELECT 1 FROM search_units u
                 WHERE u.vector_input_hash = c.vector_input_hash
             )",
            "EXPLAIN QUERY PLAN
             DELETE FROM search_vector_cache
             WHERE vector_input_hash IN (
                 SELECT c.vector_input_hash FROM search_vector_cache c
                 WHERE NOT EXISTS (
                     SELECT 1 FROM search_units u
                     WHERE u.vector_input_hash = c.vector_input_hash
                 )
                 ORDER BY c.last_used_at ASC, c.vector_input_hash ASC
                 LIMIT 1
             )",
        ];
        for statement in plans {
            let details = sqlx::query(statement)
                .fetch_all(&pool)
                .await
                .unwrap()
                .into_iter()
                .map(|row| row.get::<String, _>("detail"))
                .collect::<Vec<_>>();
            assert!(
                details
                    .iter()
                    .any(|detail| detail.contains("idx_search_units_vector_input_hash")),
                "vector-cache GC must use the vector input hash index: {details:?}"
            );
        }
    }

    #[tokio::test]
    async fn vector_cache_validates_revision_dimensions_and_payload() {
        let pool = memory_pool().await;
        migrate_project_index(&pool).await.unwrap();
        let hash = vector_input_hash("embedding.v1", 3, "path\nheading\nbody");
        persist_cached_vectors(
            &pool,
            "embedding.v1",
            3,
            &[(hash.clone(), vec![1.0, 0.0, 0.0])],
        )
        .await
        .unwrap();

        assert_eq!(
            load_cached_vectors(&pool, std::slice::from_ref(&hash), "embedding.v1", 3)
                .await
                .unwrap()
                .get(&hash),
            Some(&vec![1.0, 0.0, 0.0])
        );
        assert!(
            load_cached_vectors(&pool, std::slice::from_ref(&hash), "embedding.v2", 3)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            load_cached_vectors(&pool, std::slice::from_ref(&hash), "embedding.v1", 4)
                .await
                .unwrap()
                .is_empty()
        );

        sqlx::query("UPDATE search_vector_cache SET vector = x'0001'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            load_cached_vectors(&pool, std::slice::from_ref(&hash), "embedding.v1", 3)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn vector_cache_pruning_keeps_referenced_and_recent_unused_entries() {
        let pool = memory_pool().await;
        migrate_project_index(&pool).await.unwrap();
        for (hash, used_at) in [
            ("protected", "2026-01-01T00:00:00Z"),
            ("stale", "2026-01-02T00:00:00Z"),
            ("recent", "2026-01-03T00:00:00Z"),
        ] {
            sqlx::query(
                "INSERT INTO search_vector_cache (
                    vector_input_hash, embedding_revision, input_version,
                    dimensions, vector, last_used_at
                 ) VALUES ($1, 'embedding.v1', $2, 1, $3, $4)",
            )
            .bind(hash)
            .bind(VECTOR_INPUT_VERSION)
            .bind(encode_vector(&[1.0]))
            .bind(used_at)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO search_revisions (
                revision_id, project_id, effective_hash, model_revision,
                embedding_revision, dimensions, parser_version,
                chunker_version, ranking_version, status
             ) VALUES ('revision', 'project', 'effective', 'model',
                       'embedding.v1', 1, $1, $2, $3, 'ready')",
        )
        .bind(PARSER_VERSION)
        .bind(CHUNKER_VERSION)
        .bind(RANKING_CONFIG_VERSION)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO search_resources (
                revision_id, resource_id, project_id, scope, kind, path,
                title, content, content_hash
             ) VALUES ('revision', 'resource', 'project', 'project', 'context',
                       'context.md', 'Context', 'body', 'content')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO search_units (
                revision_id, unit_key, resource_id, ordinal,
                heading_path_json, locator_json, text, text_hash,
                token_count, vector, vector_input_hash
             ) VALUES ('revision', 'unit', 'resource', 0, '[]', '{}',
                       'body', 'text', 1, $1, 'protected')",
        )
        .bind(encode_vector(&[1.0]))
        .execute(&pool)
        .await
        .unwrap();

        let mut tx = pool.begin().await.unwrap();
        prune_unused_vector_cache(&mut tx, 1).await.unwrap();
        tx.commit().await.unwrap();

        let hashes: Vec<String> = sqlx::query_scalar(
            "SELECT vector_input_hash FROM search_vector_cache
             ORDER BY vector_input_hash",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(hashes, vec!["protected", "recent"]);
    }

    #[test]
    fn vector_input_hash_covers_every_semantic_input() {
        let base = vector_input_hash("embedding.v1", 384, "path\nheading\nbody");
        assert_eq!(
            base,
            vector_input_hash("embedding.v1", 384, "path\nheading\nbody")
        );
        assert_ne!(
            base,
            vector_input_hash("embedding.v2", 384, "path\nheading\nbody")
        );
        assert_ne!(
            base,
            vector_input_hash("embedding.v1", 768, "path\nheading\nbody")
        );
        assert_ne!(
            base,
            vector_input_hash("embedding.v1", 384, "renamed\nheading\nbody")
        );
        assert_ne!(
            base,
            vector_input_hash("embedding.v1", 384, "path\nheading\nchanged")
        );
    }

    #[tokio::test]
    async fn migrate_rebuilds_legacy_schema_six_without_description_column() {
        // Reproduces the pre-232eaac database flavor: schema_version 6 with a
        // search_resources table that lacks the description column and still
        // checks kind against the legacy values. The daemon's index build
        // failed on these with "table search_resources has no column named
        // description" because the schema version had not been bumped when
        // the column was added.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.sqlite");
        {
            let legacy = SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(
                    SqliteConnectOptions::from_str(&path.display().to_string())
                        .unwrap()
                        .create_if_missing(true),
                )
                .await
                .unwrap();
            sqlx::query(
                "CREATE TABLE search_meta (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                )",
            )
            .execute(&legacy)
            .await
            .unwrap();
            sqlx::query("INSERT INTO search_meta (key, value) VALUES ('schema_version', '6')")
                .execute(&legacy)
                .await
                .unwrap();
            sqlx::query(
                "CREATE TABLE search_resources (
                    revision_id TEXT NOT NULL,
                    resource_id TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    scope TEXT NOT NULL,
                    kind TEXT NOT NULL CHECK (kind IN ('context', 'rule', 'workflow')),
                    path TEXT NOT NULL,
                    title TEXT NOT NULL,
                    content TEXT NOT NULL,
                    content_hash TEXT NOT NULL,
                    source_commit_id TEXT,
                    draft_id TEXT,
                    draft_revision TEXT,
                    PRIMARY KEY (revision_id, resource_id)
                )",
            )
            .execute(&legacy)
            .await
            .unwrap();
            legacy.close().await;
        }

        let pool = connect_project_index(&path).await.unwrap();
        let has_description: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('search_resources')
             WHERE name = 'description'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(has_description, 1);
        let create_sql: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'search_resources'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            create_sql.contains("'memory'"),
            "kind CHECK must accept memory"
        );
        let version: String =
            sqlx::query_scalar("SELECT value FROM search_meta WHERE key = 'schema_version'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(version, PROJECT_INDEX_SCHEMA_VERSION.to_string());
        pool.close().await;
    }
}
