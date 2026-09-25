use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use tokio::task::JoinHandle;

use super::index::PrepareIndexOutcome;
use super::{DaemonError, DaemonState, SearchFailure};

const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(30);
const MUTATION_COALESCE_WINDOW: Duration = Duration::from_millis(25);
const CLAIM_RETRY_MIN_BACKOFF: Duration = Duration::from_millis(10);
const CLAIM_RETRY_MAX_BACKOFF: Duration = Duration::from_secs(1);
const FAILURE_RETRY_MIN_BACKOFF: Duration = Duration::from_millis(250);
const FAILURE_RETRY_MAX_BACKOFF: Duration = Duration::from_secs(30);
const RETAIN_READY_REVISIONS: usize = 3;

fn now_epoch_millis() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

fn failure_retry_delay(attempt: i64) -> Duration {
    let exponent = attempt.saturating_sub(1).clamp(0, 31) as u32;
    FAILURE_RETRY_MIN_BACKOFF
        .saturating_mul(1_u32 << exponent)
        .min(FAILURE_RETRY_MAX_BACKOFF)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SearchIndexJobStatus {
    pub(crate) project_id: String,
    pub(crate) desired_sequence: i64,
    pub(crate) completed_sequence: i64,
    pub(crate) state: String,
    pub(crate) target_effective_hash: Option<String>,
    pub(crate) active_revision: Option<String>,
    pub(crate) last_error: Option<String>,
}

struct ClaimedJob {
    project_id: String,
    sequence: i64,
    attempt: i64,
}

pub(crate) async fn migrate(pool: &SqlitePool) -> Result<(), DaemonError> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS search_index_jobs (
            project_id TEXT PRIMARY KEY,
            desired_sequence BIGINT NOT NULL CHECK (desired_sequence > 0),
            building_sequence BIGINT,
            completed_sequence BIGINT NOT NULL DEFAULT 0 CHECK (completed_sequence >= 0),
            state TEXT NOT NULL CHECK (state IN ('queued', 'building', 'ready', 'failed')),
            target_effective_hash TEXT,
            active_revision TEXT,
            last_error TEXT,
            attempt BIGINT NOT NULL DEFAULT 0 CHECK (attempt >= 0),
            next_retry_at_ms BIGINT,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
    )
    .execute(pool)
    .await?;
    let columns = sqlx::query("PRAGMA table_info(search_index_jobs)")
        .fetch_all(pool)
        .await?;
    if !columns
        .iter()
        .any(|row| row.get::<String, _>("name") == "next_retry_at_ms")
    {
        sqlx::query("ALTER TABLE search_index_jobs ADD COLUMN next_retry_at_ms BIGINT")
            .execute(pool)
            .await?;
    }
    sqlx::query(
        "UPDATE search_index_jobs
         SET state = 'queued', building_sequence = NULL,
             next_retry_at_ms = NULL,
             last_error = CASE
                 WHEN state = 'building' THEN 'daemon restarted during index construction'
                 ELSE last_error
             END,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE state = 'building'
            OR (state = 'ready' AND completed_sequence < desired_sequence)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE search_index_jobs
         SET next_retry_at_ms = COALESCE(next_retry_at_ms, $1)
         WHERE state = 'failed' AND completed_sequence < desired_sequence",
    )
    .bind(now_epoch_millis())
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn enqueue_project_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
) -> Result<(), DaemonError> {
    let project_id = project_id.trim();
    if project_id.is_empty() {
        return Err(DaemonError::InvalidRequest(
            "project_id must not be empty when scheduling a search index".to_owned(),
        ));
    }
    sqlx::query(
        "INSERT INTO search_index_jobs (
            project_id, desired_sequence, completed_sequence, state
         ) VALUES ($1, 1, 0, 'queued')
         ON CONFLICT(project_id) DO UPDATE SET
            desired_sequence = search_index_jobs.desired_sequence + 1,
            state = 'queued',
            last_error = NULL,
            attempt = 0,
            next_retry_at_ms = NULL,
            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
    )
    .bind(project_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn enqueue_all_cached_projects_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
) -> Result<(), DaemonError> {
    let projects = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT project_id
         FROM cached_refs
         WHERE scope = 'project' AND project_id IS NOT NULL
         ORDER BY project_id",
    )
    .fetch_all(&mut **tx)
    .await?;
    for project_id in projects {
        enqueue_project_in_tx(tx, &project_id).await?;
    }
    Ok(())
}

pub(crate) async fn enqueue_project(
    state: &DaemonState,
    project_id: &str,
) -> Result<(), DaemonError> {
    let mut tx = state.inner.pool.begin().await?;
    enqueue_project_in_tx(&mut tx, project_id).await?;
    tx.commit().await?;
    state.inner.search_index_notify.notify_one();
    Ok(())
}

pub(crate) async fn ensure_project_queued(
    state: &DaemonState,
    project_id: &str,
) -> Result<(), DaemonError> {
    let project_id = project_id.trim();
    if project_id.is_empty() {
        return Err(DaemonError::InvalidRequest(
            "project_id must not be empty when scheduling a search index".to_owned(),
        ));
    }
    sqlx::query(
        "INSERT INTO search_index_jobs (
            project_id, desired_sequence, completed_sequence, state
         ) VALUES ($1, 1, 0, 'queued')
         ON CONFLICT(project_id) DO UPDATE SET
            desired_sequence = CASE
                WHEN search_index_jobs.state = 'ready'
                 AND search_index_jobs.completed_sequence >= search_index_jobs.desired_sequence
                THEN search_index_jobs.desired_sequence + 1
                ELSE search_index_jobs.desired_sequence
            END,
            state = CASE
                WHEN search_index_jobs.state = 'ready'
                 AND search_index_jobs.completed_sequence >= search_index_jobs.desired_sequence
                THEN 'queued'
                ELSE search_index_jobs.state
            END,
            attempt = CASE
                WHEN search_index_jobs.state = 'ready'
                 AND search_index_jobs.completed_sequence >= search_index_jobs.desired_sequence
                THEN 0
                ELSE search_index_jobs.attempt
            END,
            next_retry_at_ms = CASE
                WHEN search_index_jobs.state = 'ready'
                 AND search_index_jobs.completed_sequence >= search_index_jobs.desired_sequence
                THEN NULL
                ELSE search_index_jobs.next_retry_at_ms
            END,
            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
    )
    .bind(project_id)
    .execute(&state.inner.pool)
    .await?;
    state.inner.search_index_notify.notify_one();
    Ok(())
}

pub(crate) async fn enqueue_all_cached_projects(state: &DaemonState) -> Result<(), DaemonError> {
    let mut tx = state.inner.pool.begin_with("BEGIN IMMEDIATE").await?;
    enqueue_all_cached_projects_in_tx(&mut tx).await?;
    tx.commit().await?;
    state.inner.search_index_notify.notify_one();
    Ok(())
}

pub(crate) async fn status(
    state: &DaemonState,
    project_id: &str,
) -> Result<Option<SearchIndexJobStatus>, DaemonError> {
    let row = sqlx::query(
        "SELECT project_id, desired_sequence, completed_sequence, state,
                target_effective_hash, active_revision, last_error
         FROM search_index_jobs WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(&state.inner.pool)
    .await?;
    row.map(|row| {
        Ok(SearchIndexJobStatus {
            project_id: row.try_get("project_id")?,
            desired_sequence: row.try_get("desired_sequence")?,
            completed_sequence: row.try_get("completed_sequence")?,
            state: row.try_get("state")?,
            target_effective_hash: row.try_get("target_effective_hash")?,
            active_revision: row.try_get("active_revision")?,
            last_error: row.try_get("last_error")?,
        })
    })
    .transpose()
}

pub(crate) fn start_worker(state: &DaemonState) -> JoinHandle<()> {
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(error) = seed_cached_projects(&state.inner.pool).await {
            tracing::warn!("failed to seed background search index jobs: {error}");
        }
        let mut interval = tokio::time::interval(IDLE_POLL_INTERVAL);
        loop {
            let retry_delay = match next_failed_retry_delay(&state.inner.pool).await {
                Ok(delay) => delay,
                Err(error) => {
                    tracing::warn!("failed to read the next search index retry deadline: {error}");
                    None
                }
            };
            let mutation_notification = if let Some(retry_delay) = retry_delay {
                tokio::select! {
                    _ = interval.tick() => false,
                    _ = state.inner.search_index_notify.notified() => true,
                    _ = tokio::time::sleep(retry_delay) => false,
                }
            } else {
                tokio::select! {
                    _ = interval.tick() => false,
                    _ = state.inner.search_index_notify.notified() => true,
                }
            };
            if mutation_notification {
                // Build only after a short quiet period so a burst of Draft
                // or Ref commits becomes one latest-target job.
                while tokio::time::timeout(
                    MUTATION_COALESCE_WINDOW,
                    state.inner.search_index_notify.notified(),
                )
                .await
                .is_ok()
                {}
            }
            let mut retry_backoff = CLAIM_RETRY_MIN_BACKOFF;
            loop {
                match claim_next_job(&state.inner.pool).await {
                    Ok(Some(job)) => {
                        retry_backoff = CLAIM_RETRY_MIN_BACKOFF;
                        if let Err(error) = build_claimed_job(&state, &job).await {
                            tracing::warn!(
                                project_id = %job.project_id,
                                sequence = job.sequence,
                                "background search index build failed: {error}"
                            );
                            persist_failed_job(&state.inner.pool, &job, &error).await;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        tracing::warn!(
                            retry_ms = retry_backoff.as_millis(),
                            "failed to claim a background search index job; retrying: {error}"
                        );
                        tokio::time::sleep(retry_backoff).await;
                        retry_backoff =
                            retry_backoff.saturating_mul(2).min(CLAIM_RETRY_MAX_BACKOFF);
                    }
                }
            }
        }
    })
}

async fn persist_failed_job(pool: &SqlitePool, job: &ClaimedJob, error: &DaemonError) {
    let mut retry_backoff = CLAIM_RETRY_MIN_BACKOFF;
    loop {
        match mark_failed(pool, job, error).await {
            Ok(()) => return,
            Err(mark_error) => {
                tracing::warn!(
                    project_id = %job.project_id,
                    sequence = job.sequence,
                    retry_ms = retry_backoff.as_millis(),
                    "failed to persist background search index failure; retrying: {mark_error}"
                );
                tokio::time::sleep(retry_backoff).await;
                retry_backoff = retry_backoff.saturating_mul(2).min(CLAIM_RETRY_MAX_BACKOFF);
            }
        }
    }
}

async fn next_failed_retry_delay(pool: &SqlitePool) -> Result<Option<Duration>, DaemonError> {
    let retry_at_ms: Option<i64> = sqlx::query_scalar(
        "SELECT MIN(next_retry_at_ms)
         FROM search_index_jobs
         WHERE state = 'failed' AND completed_sequence < desired_sequence",
    )
    .fetch_one(pool)
    .await?;
    Ok(retry_at_ms.map(|retry_at_ms| {
        Duration::from_millis(retry_at_ms.saturating_sub(now_epoch_millis()).max(0) as u64)
    }))
}

async fn seed_cached_projects(pool: &SqlitePool) -> Result<(), DaemonError> {
    sqlx::query(
        "INSERT INTO search_index_jobs (
            project_id, desired_sequence, completed_sequence, state
         )
         SELECT DISTINCT project_id, 1, 0, 'queued'
         FROM cached_refs
         WHERE scope = 'project' AND project_id IS NOT NULL
         ON CONFLICT(project_id) DO NOTHING",
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn claim_next_job(pool: &SqlitePool) -> Result<Option<ClaimedJob>, DaemonError> {
    // Keep selection and transition in one write statement. A deferred
    // SELECT-then-UPDATE transaction can fail with SQLITE_BUSY_SNAPSHOT when
    // another central-database writer commits between those statements.
    let row = sqlx::query(
        "UPDATE search_index_jobs
         SET state = 'building', building_sequence = desired_sequence,
             target_effective_hash = NULL, last_error = NULL,
             attempt = attempt + 1, next_retry_at_ms = NULL,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE project_id = (
             SELECT project_id
             FROM search_index_jobs
             WHERE completed_sequence < desired_sequence
               AND (
                   state = 'queued'
                   OR (state = 'failed' AND next_retry_at_ms <= $1)
               )
             ORDER BY updated_at, project_id
             LIMIT 1
         )
           AND completed_sequence < desired_sequence
           AND (
               state = 'queued'
               OR (state = 'failed' AND next_retry_at_ms <= $1)
           )
         RETURNING project_id, desired_sequence, attempt",
    )
    .bind(now_epoch_millis())
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        Ok(ClaimedJob {
            project_id: row.try_get("project_id")?,
            sequence: row.try_get("desired_sequence")?,
            attempt: row.try_get("attempt")?,
        })
    })
    .transpose()
}

async fn build_claimed_job(state: &DaemonState, job: &ClaimedJob) -> Result<(), DaemonError> {
    let effective = match super::load_effective_memory(state, &job.project_id).await {
        Ok(effective) => effective,
        Err(DaemonError::State {
            code: "project_ref_not_synced",
            ..
        }) => {
            return Err(SearchFailure::index_preparing(
                "Project Ref is not synchronized; the search build remains queued",
            )
            .into());
        }
        Err(error) => return Err(error),
    };
    sqlx::query(
        "UPDATE search_index_jobs SET target_effective_hash = $3
         WHERE project_id = $1 AND building_sequence = $2 AND state = 'building'",
    )
    .bind(&job.project_id)
    .bind(job.sequence)
    .bind(&effective.effective_hash)
    .execute(&state.inner.pool)
    .await?;

    let storage_guard = state.inner.storage_access.read().await;
    let (pool, storage) = super::active_project_index(state, &job.project_id).await?;
    let prepared = super::index::prepare_incremental_index(state, &pool, &effective, || {
        let state = state.clone();
        let project_id = job.project_id.clone();
        let sequence = job.sequence;
        async move { job_is_current(&state.inner.pool, &project_id, sequence).await }
    })
    .await?;

    match prepared {
        PrepareIndexOutcome::Superseded => {
            pool.close().await;
            mark_superseded(&state.inner.pool, job).await?;
            return Ok(());
        }
        PrepareIndexOutcome::AlreadyReady(revision_id) => {
            publish_ready_job(
                state,
                job,
                &pool,
                storage.location_revision,
                &effective,
                &revision_id,
            )
            .await?;
        }
        PrepareIndexOutcome::Prepared(prepared) => {
            // Persist the large revision outside the central writer barrier;
            // it remains invisible until the small head switch below.
            let revision_id =
                super::index::stage_prepared_index(&pool, &effective, &prepared).await?;
            publish_ready_job(
                state,
                job,
                &pool,
                storage.location_revision,
                &effective,
                &revision_id,
            )
            .await?;
        }
    }
    pool.close().await;
    drop(storage_guard);
    prune_old_revisions(state, &job.project_id).await?;
    Ok(())
}

pub(super) async fn prune_old_revisions(
    state: &DaemonState,
    project_id: &str,
) -> Result<(), DaemonError> {
    // Activate holds this lock from head selection through query completion,
    // so an old revision cannot be removed from under an in-flight reader.
    let _query_guard = state.inner.search_lock.lock().await;
    // Match every other search/storage operation's lock order. Reopen the
    // current active index because a storage move may have completed after
    // publication released its original storage pin.
    let _storage_guard = state.inner.storage_access.read().await;
    let (pool, _storage) = super::active_project_index(state, project_id).await?;
    let result =
        super::index::prune_old_ready_revisions(&pool, project_id, RETAIN_READY_REVISIONS).await;
    pool.close().await;
    result
}

#[allow(clippy::too_many_arguments)]
async fn publish_ready_job(
    state: &DaemonState,
    job: &ClaimedJob,
    project_pool: &SqlitePool,
    location_revision: i64,
    effective: &super::EffectiveMemory,
    revision_id: &str,
) -> Result<(), DaemonError> {
    let mut barrier = state.inner.pool.begin().await?;
    // This no-op write acquires SQLite's single-writer barrier. Every
    // supported Effective Memory mutation increments desired_sequence in its
    // own transaction, so no mutation can commit after this CAS until both
    // search heads and the job row have been published.
    let current = sqlx::query(
        "UPDATE search_index_jobs
         SET updated_at = updated_at
         WHERE project_id = $1 AND desired_sequence = $2
           AND building_sequence = $2 AND state = 'building'",
    )
    .bind(&job.project_id)
    .bind(job.sequence)
    .execute(&mut *barrier)
    .await?
    .rows_affected()
        == 1;
    if !current {
        barrier.rollback().await?;
        mark_superseded(&state.inner.pool, job).await?;
        return Ok(());
    }

    super::index::publish_staged_index(
        project_pool,
        &job.project_id,
        &effective.effective_hash,
        revision_id,
    )
    .await?;
    upsert_search_head_in_tx(
        &mut barrier,
        &job.project_id,
        revision_id,
        &effective.effective_hash,
        location_revision,
    )
    .await?;
    mark_ready_in_tx(&mut barrier, job, &effective.effective_hash, revision_id).await?;
    barrier.commit().await?;
    Ok(())
}

async fn upsert_search_head_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    revision_id: &str,
    effective_hash: &str,
    location_revision: i64,
) -> Result<(), DaemonError> {
    sqlx::query(
        "INSERT INTO search_heads (
            project_id, revision_id, effective_hash, status, last_error, location_revision
         ) VALUES ($1, $2, $3, 'ready', NULL, $4)
         ON CONFLICT(project_id) DO UPDATE SET
            revision_id = excluded.revision_id,
            effective_hash = excluded.effective_hash,
            status = 'ready',
            last_error = NULL,
            location_revision = excluded.location_revision,
            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
    )
    .bind(project_id)
    .bind(revision_id)
    .bind(effective_hash)
    .bind(location_revision)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn job_is_current(pool: &SqlitePool, project_id: &str, sequence: i64) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM search_index_jobs
         WHERE project_id = $1 AND desired_sequence = $2
           AND building_sequence = $2 AND state = 'building'",
    )
    .bind(project_id)
    .bind(sequence)
    .fetch_one(pool)
    .await
    .is_ok_and(|count| count == 1)
}

async fn mark_ready_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    job: &ClaimedJob,
    effective_hash: &str,
    revision_id: &str,
) -> Result<(), DaemonError> {
    let updated = sqlx::query(
        "UPDATE search_index_jobs
         SET completed_sequence = $2, building_sequence = NULL, state = 'ready',
             target_effective_hash = $3, active_revision = $4, last_error = NULL,
             attempt = 0, next_retry_at_ms = NULL,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE project_id = $1 AND desired_sequence = $2 AND building_sequence = $2",
    )
    .bind(&job.project_id)
    .bind(job.sequence)
    .bind(effective_hash)
    .bind(revision_id)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if updated != 1 {
        return Err(SearchFailure::generation_changed(
            "search index target changed before publication",
        )
        .into());
    }
    Ok(())
}

async fn mark_superseded(pool: &SqlitePool, job: &ClaimedJob) -> Result<(), DaemonError> {
    sqlx::query(
        "UPDATE search_index_jobs
         SET state = 'queued', building_sequence = NULL,
             next_retry_at_ms = NULL,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE project_id = $1 AND building_sequence = $2",
    )
    .bind(&job.project_id)
    .bind(job.sequence)
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_failed(
    pool: &SqlitePool,
    job: &ClaimedJob,
    error: &DaemonError,
) -> Result<(), DaemonError> {
    let message = error.to_string();
    let retry_at_ms = now_epoch_millis().saturating_add(
        i64::try_from(failure_retry_delay(job.attempt).as_millis()).unwrap_or(i64::MAX),
    );
    sqlx::query(
        "UPDATE search_index_jobs
         SET state = CASE WHEN desired_sequence = $2 THEN 'failed' ELSE 'queued' END,
             building_sequence = NULL, last_error = $3,
             next_retry_at_ms = CASE
                 WHEN desired_sequence = $2 THEN $4
                 ELSE NULL
             END,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE project_id = $1 AND building_sequence = $2",
    )
    .bind(&job.project_id)
    .bind(job.sequence)
    .bind(message)
    .bind(retry_at_ms)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

    use super::*;

    async fn pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE cached_refs (
                ref_key TEXT PRIMARY KEY, scope TEXT NOT NULL, project_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        migrate(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn enqueue_coalesces_to_the_latest_sequence() {
        let pool = pool().await;
        let mut tx = pool.begin().await.unwrap();
        enqueue_project_in_tx(&mut tx, "prj_test").await.unwrap();
        enqueue_project_in_tx(&mut tx, "prj_test").await.unwrap();
        tx.commit().await.unwrap();

        let row = sqlx::query("SELECT desired_sequence, state FROM search_index_jobs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.get::<i64, _>("desired_sequence"), 2);
        assert_eq!(row.get::<String, _>("state"), "queued");
    }

    #[tokio::test]
    async fn claim_atomically_transitions_the_latest_sequence() {
        let pool = pool().await;
        let mut tx = pool.begin().await.unwrap();
        enqueue_project_in_tx(&mut tx, "prj_test").await.unwrap();
        enqueue_project_in_tx(&mut tx, "prj_test").await.unwrap();
        tx.commit().await.unwrap();

        let claimed = claim_next_job(&pool).await.unwrap().unwrap();
        assert_eq!(claimed.project_id, "prj_test");
        assert_eq!(claimed.sequence, 2);
        assert!(claim_next_job(&pool).await.unwrap().is_none());

        let row = sqlx::query(
            "SELECT desired_sequence, building_sequence, state, attempt
             FROM search_index_jobs WHERE project_id = 'prj_test'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.get::<i64, _>("desired_sequence"), 2);
        assert_eq!(row.get::<i64, _>("building_sequence"), 2);
        assert_eq!(row.get::<String, _>("state"), "building");
        assert_eq!(row.get::<i64, _>("attempt"), 1);
    }

    #[tokio::test]
    async fn failed_job_retries_only_after_its_bounded_backoff() {
        let pool = pool().await;
        let mut tx = pool.begin().await.unwrap();
        enqueue_project_in_tx(&mut tx, "prj_test").await.unwrap();
        tx.commit().await.unwrap();

        let first = claim_next_job(&pool).await.unwrap().unwrap();
        let failure = DaemonError::Search {
            code: "search_index_failed".to_owned(),
            message: "transient model failure".to_owned(),
        };
        mark_failed(&pool, &first, &failure).await.unwrap();
        assert!(claim_next_job(&pool).await.unwrap().is_none());
        let retry_delay = next_failed_retry_delay(&pool).await.unwrap().unwrap();
        assert!(retry_delay <= FAILURE_RETRY_MIN_BACKOFF);

        sqlx::query("UPDATE search_index_jobs SET next_retry_at_ms = $2 WHERE project_id = $1")
            .bind("prj_test")
            .bind(now_epoch_millis().saturating_sub(1))
            .execute(&pool)
            .await
            .unwrap();
        let retry = claim_next_job(&pool).await.unwrap().unwrap();
        assert_eq!(retry.project_id, "prj_test");
        assert_eq!(retry.sequence, first.sequence);
        assert_eq!(retry.attempt, 2);
        assert_eq!(
            failure_retry_delay(retry.attempt),
            Duration::from_millis(500)
        );
    }

    #[tokio::test]
    async fn writer_contention_leaves_a_job_claimable_after_retry() {
        let temp = tempfile::tempdir().unwrap();
        let options = SqliteConnectOptions::new()
            .filename(temp.path().join("scheduler.sqlite"))
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_millis(1));
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE cached_refs (
                ref_key TEXT PRIMARY KEY, scope TEXT NOT NULL, project_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        migrate(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        enqueue_project_in_tx(&mut tx, "prj_test").await.unwrap();
        tx.commit().await.unwrap();

        let mut writer = pool.begin().await.unwrap();
        sqlx::query(
            "UPDATE search_index_jobs SET updated_at = updated_at WHERE project_id = 'prj_test'",
        )
        .execute(&mut *writer)
        .await
        .unwrap();
        assert!(claim_next_job(&pool).await.is_err());
        writer.rollback().await.unwrap();

        let claimed = claim_next_job(&pool).await.unwrap().unwrap();
        assert_eq!(claimed.project_id, "prj_test");
        assert_eq!(claimed.sequence, 1);
        assert_eq!(claimed.attempt, 1);
    }

    #[tokio::test]
    async fn migration_recovers_interrupted_builds() {
        let pool = pool().await;
        sqlx::query(
            "INSERT INTO search_index_jobs (
                project_id, desired_sequence, building_sequence,
                completed_sequence, state
             ) VALUES ('prj_test', 3, 3, 1, 'building')",
        )
        .execute(&pool)
        .await
        .unwrap();
        migrate(&pool).await.unwrap();
        let row = sqlx::query("SELECT state, building_sequence FROM search_index_jobs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("state"), "queued");
        assert!(row.get::<Option<i64>, _>("building_sequence").is_none());
    }

    #[tokio::test]
    async fn all_project_enqueue_ignores_org_refs_and_duplicates() {
        let pool = pool().await;
        for (key, scope, project) in [
            ("project:a", "project", Some("prj_a")),
            ("project:b", "project", Some("prj_b")),
            ("project:b2", "project", Some("prj_b")),
            ("org:o", "org", None),
        ] {
            sqlx::query("INSERT INTO cached_refs (ref_key, scope, project_id) VALUES ($1, $2, $3)")
                .bind(key)
                .bind(scope)
                .bind(project)
                .execute(&pool)
                .await
                .unwrap();
        }
        let mut tx = pool.begin().await.unwrap();
        enqueue_all_cached_projects_in_tx(&mut tx).await.unwrap();
        tx.commit().await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM search_index_jobs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 2);
    }
}
