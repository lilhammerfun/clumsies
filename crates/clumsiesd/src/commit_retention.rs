//! Reclaim unreachable Commit objects while preserving drafts and live readers.

use std::collections::{BTreeSet, VecDeque};

use serde::Deserialize;
use sqlx::{Sqlite, Transaction};

use crate::types::{DaemonDraftOperation, OrgMemorySource};
use crate::{DaemonError, DaemonState, commit_sync, migration};

// The caller holds sync_lock and storage_access.write(): no Base fetch or cache
// reader can cross the atomic mark/sweep boundary. BEGIN IMMEDIATE also serializes
// ordinary Draft creation and operation insertion with the roots snapshot.
/// Reclaim unreachable immutable objects while the caller holds both storage guards.
///
/// Draft dependencies and live reader pins are captured in one write transaction;
/// incomplete retained dependencies abort deletion. Committed free pages are reclaimed.
///
/// # Errors
/// Returns storage, dependency, pin-file, or page-reclamation errors. Transaction
/// errors preserve the cache; reclamation errors can follow a committed safe sweep.
pub(super) async fn prune_cached_commits_under_storage_guard(
    state: &DaemonState,
) -> Result<(), DaemonError> {
    let mut tx = state.inner.pool.begin_with("BEGIN IMMEDIATE").await?;
    let pins = commit_sync::live_commit_base_pins(state, &mut tx).await?;
    let deleted = prune_in_transaction(&mut tx, pins).await?;
    tx.commit().await?;
    commit_sync::cleanup_commit_base_pin_files(state).await?;
    let free_pages: i64 = sqlx::query_scalar("PRAGMA freelist_count")
        .fetch_one(&state.inner.pool)
        .await?;
    if deleted || free_pages > 0 {
        migration::reclaim_local_db_pages(&state.inner.pool).await?;
    }
    Ok(())
}

/// Mark draft and reader dependencies, then sweep only objects outside that set.
///
/// # Errors
/// Returns database or dependency errors before the caller commits the transaction.
async fn prune_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    pins: Vec<String>,
) -> Result<bool, DaemonError> {
    let mut roots: BTreeSet<String> = pins.into_iter().collect();
    roots.extend(
        sqlx::query_scalar::<_, String>(
            "SELECT commit_id FROM cached_refs WHERE commit_id IS NOT NULL
             UNION SELECT value FROM daemon_meta
                   WHERE key GLOB 'previous_project_generation:*'
             UNION SELECT base_commit_id FROM local_drafts d
                   WHERE base_commit_id IS NOT NULL
                     AND (status IN ('open', 'submitted') OR EXISTS (
                         SELECT 1 FROM local_draft_operations o
                         WHERE o.draft_id = d.draft_id AND o.sync_status <> 'synced'))
             UNION SELECT current_commit_id FROM local_drafts d
                   WHERE current_commit_id IS NOT NULL
                     AND (status IN ('open', 'submitted') OR EXISTS (
                         SELECT 1 FROM local_draft_operations o
                         WHERE o.draft_id = d.draft_id AND o.sync_status <> 'synced'))",
        )
        .fetch_all(&mut **tx)
        .await?,
    );
    let operations: Vec<String> = sqlx::query_scalar(
        "SELECT o.operation_json FROM local_draft_operations o
         JOIN local_drafts d ON d.draft_id = o.draft_id
         WHERE d.status IN ('open', 'submitted') OR o.sync_status <> 'synced'",
    )
    .fetch_all(&mut **tx)
    .await?;
    for json in operations {
        let operation: DaemonDraftOperation = serde_json::from_str(&json)?;
        if [
            operation.create.is_some(),
            operation.update.is_some(),
            operation.rename.is_some(),
            operation.delete.is_some(),
            operation.discard.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count()
            != 1
        {
            return Err(unreadable_dependency("Draft operation"));
        }
        // Only the actual content.org_source field is a dependency. A Markdown
        // body may mention any historical Commit without needing its cache.
        if let Some(source) = operation
            .create
            .as_ref()
            .map(|create| &create.content)
            .or_else(|| {
                operation
                    .update
                    .as_ref()
                    .and_then(|update| update.content())
            })
            .and_then(|content| content.org_source.as_ref())
        {
            validate_source(source)?;
            roots.insert(source.commit_id.clone());
        }
    }

    let mut pending: VecDeque<String> = roots.into_iter().collect();
    let mut visited = BTreeSet::new();
    let mut commits = BTreeSet::new();
    let mut trees = BTreeSet::new();
    let mut blobs = BTreeSet::new();
    while let Some(commit_id) = pending.pop_front() {
        if !visited.insert(commit_id.clone()) {
            continue;
        }
        let cached: Option<(String, String)> =
            sqlx::query_as("SELECT tree_id, payload_json FROM cached_commits WHERE commit_id = $1")
                .bind(&commit_id)
                .fetch_optional(&mut **tx)
                .await?;
        let Some((tree_id, commit_json)) = cached else {
            // Offline Drafts and org_source provenance may refer to a Server
            // version never downloaded here. There is no local object to sweep.
            continue;
        };
        let commit: RetainedCommit = serde_json::from_str(&commit_json)?;
        if commit.commit_id != commit_id || commit.tree_id != tree_id || tree_id.is_empty() {
            return Err(unreadable_dependency(&commit_id));
        }
        commits.insert(commit_id);
        if !trees.insert(tree_id.clone()) {
            continue;
        }
        let tree_json: Option<String> =
            sqlx::query_scalar("SELECT payload_json FROM cached_trees WHERE tree_id = $1")
                .bind(&tree_id)
                .fetch_optional(&mut **tx)
                .await?;
        let tree: RetainedTree =
            serde_json::from_str(&tree_json.ok_or_else(|| unreadable_dependency(&tree_id))?)?;
        if tree.tree_id != tree_id {
            return Err(unreadable_dependency(&tree_id));
        }
        for entry in tree.entries {
            if entry.blob_id.is_empty() {
                return Err(unreadable_dependency(&tree_id));
            }
            blobs.insert(entry.blob_id);
            if let Some(source) = entry.org_source {
                validate_source(&source)?;
                pending.push_back(source.commit_id);
            }
        }
        // parent_commit_id is historical metadata, not a content dependency.
    }
    let blobs_json = serde_json::to_string(&blobs)?;
    let cached_blob_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM cached_blobs WHERE blob_id IN (SELECT value FROM json_each($1))",
    )
    .bind(&blobs_json)
    .fetch_one(&mut **tx)
    .await?;
    if cached_blob_count as usize != blobs.len() {
        return Err(unreadable_dependency("retained Blobs"));
    }

    // Terminal history keeps its displayed path even after its Base cache is gone.
    sqlx::query(&format!(
        "UPDATE local_drafts AS d SET path = {}
         WHERE d.status IN ('merged', 'discarded') AND NOT EXISTS (
             SELECT 1 FROM local_draft_operations o
             WHERE o.draft_id = d.draft_id AND o.sync_status <> 'synced')",
        crate::draft::DRAFT_PATH_SQL,
    ))
    .execute(&mut **tx)
    .await?;

    // All dependency parsing completes before the first deletion. Bound JSON
    // arrays avoid SQLite's bind-variable limit for large retained snapshots.
    let commits_deleted = sqlx::query(
        "DELETE FROM cached_commits WHERE commit_id NOT IN (SELECT value FROM json_each($1))",
    )
    .bind(serde_json::to_string(&commits)?)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    let trees_deleted = sqlx::query(
        "DELETE FROM cached_trees WHERE tree_id NOT IN (SELECT value FROM json_each($1))",
    )
    .bind(serde_json::to_string(&trees)?)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    let blobs_deleted = sqlx::query(
        "DELETE FROM cached_blobs WHERE blob_id NOT IN (SELECT value FROM json_each($1))",
    )
    .bind(blobs_json)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    tracing::debug!(
        commits_deleted,
        trees_deleted,
        blobs_deleted,
        "pruned historical Commit cache"
    );
    Ok(commits_deleted != 0 || trees_deleted != 0 || blobs_deleted != 0)
}

/// Report that cleanup must preserve objects whose dependencies cannot be verified.
fn unreadable_dependency(id: &str) -> DaemonError {
    DaemonError::Server(format!(
        "Commit cache cleanup skipped: dependency {id} is incomplete or unreadable"
    ))
}

/// Require both provenance identifiers before tracing an Organization source.
///
/// # Errors
/// Returns an incomplete-dependency error when either identifier is empty.
fn validate_source(source: &OrgMemorySource) -> Result<(), DaemonError> {
    if source.commit_id.is_empty() || source.resource_id.is_empty() {
        return Err(unreadable_dependency("org_source"));
    }
    Ok(())
}

// These projections read dependency metadata only; GC never rereads/hashes Blob
// bodies. The full payload is verified when it enters the immutable cache.
#[derive(Deserialize)]
/// Commit identity and root Tree required by the collection traversal.
struct RetainedCommit {
    /// Immutable Commit key checked against the cached row.
    commit_id: String,
    /// Root Tree containing the retained content dependencies.
    tree_id: String,
}

#[derive(Deserialize)]
/// Root Tree projection used to follow retained content dependencies.
struct RetainedTree {
    /// Immutable Tree key checked against the cached row.
    tree_id: String,
    /// Entries whose content and provenance must remain available.
    entries: Vec<RetainedTreeEntry>,
}

#[derive(Deserialize)]
/// Content and Organization provenance dependencies of one retained entry.
struct RetainedTreeEntry {
    /// Immutable body that remains reachable through the Tree.
    blob_id: String,
    /// Organization Commit required to reconstruct adapted content.
    org_source: Option<OrgMemorySource>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use sqlx::SqlitePool;

    async fn pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        migration::migrate_local_db(&pool).await.unwrap();
        pool
    }

    async fn cache_commit(
        pool: &SqlitePool,
        id: &str,
        parent: Option<&str>,
        org_source: Option<&str>,
        body: &str,
    ) -> String {
        let hash_blob = |body: &str| {
            let mut hash = Sha256::new();
            hash.update(b"blob\0");
            hash.update(body.as_bytes());
            hex::encode(hash.finalize())
        };
        let blob_id = hash_blob(body);
        let selection = json!({"project_id": "p", "memories": [], "revision": 0}).to_string();
        let selection_blob = hash_blob(&selection);
        for (blob, content) in [(&blob_id, body), (&selection_blob, selection.as_str())] {
            sqlx::query("INSERT OR IGNORE INTO cached_blobs (blob_id, content) VALUES ($1, $2)")
                .bind(blob)
                .bind(content)
                .execute(pool)
                .await
                .unwrap();
        }
        let tree_id = format!("tree_{id}");
        let mut memory = json!({
            "id": "mem_one", "type": "memory", "scope": "project", "project_id": "p",
            "path": "memory.md", "blob_id": blob_id, "source": "project", "description": ""
        });
        if let Some(source) = org_source {
            memory["org_source"] = json!({"resource_id": "mem_one", "commit_id": source});
        }
        let tree = json!({"tree_id": tree_id, "entries": [memory, {
            "id": "selection_p", "type": "project_org_selection", "scope": "daemon",
            "project_id": "p", "path": null, "blob_id": selection_blob, "source": "config"
        }]});
        sqlx::query("INSERT INTO cached_trees (tree_id, payload_json) VALUES ($1, $2)")
            .bind(&tree_id)
            .bind(tree.to_string())
            .execute(pool)
            .await
            .unwrap();
        let commit = json!({
            "commit_id": id, "scope": "project", "org_id": "org", "project_id": "p",
            "tree_id": tree_id, "parent_commit_id": parent, "version": 1,
            "created_at": "2026-10-12T00:00:00Z"
        });
        sqlx::query(
            "INSERT INTO cached_commits (commit_id, scope, org_id, project_id, tree_id,
             parent_commit_id, version, created_at, payload_json)
             VALUES ($1, 'project', 'org', 'p', $2, $3, 1, '2026-10-12T00:00:00Z', $4)",
        )
        .bind(id)
        .bind(tree_id)
        .bind(parent)
        .bind(commit.to_string())
        .execute(pool)
        .await
        .unwrap();
        blob_id
    }

    async fn set_ref(pool: &SqlitePool, id: &str) {
        sqlx::query(
            "INSERT INTO cached_refs (ref_key, name, scope, org_id, project_id,
             commit_id, etag, server_updated_at)
             VALUES ('project:p', 'main', 'project', 'org', 'p', $1, 'e', 't')",
        )
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn draft(pool: &SqlitePool, id: &str, base: &str, current: &str, status: &str) {
        sqlx::query(
            "INSERT INTO local_drafts (draft_id, project_id, base_commit_id, current_commit_id,
             resource_scope, resource_kind, status) VALUES ($1, 'p', $2, $3, 'project', 'memory', $4)",
        )
        .bind(id)
        .bind(base)
        .bind(current)
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn operation(pool: &SqlitePool, draft: &str, status: &str, source: Option<&str>) {
        let mut content = json!({"content": "Markdown mentioning {\"org_source\":{\"commit_id\":\"body_mention\"}}"});
        if let Some(source) = source {
            content["org_source"] = json!({"resource_id": "mem_one", "commit_id": source});
        }
        sqlx::query(
            "INSERT INTO local_draft_operations (local_operation_id, draft_id, resource_kind,
             operation_json, source, sync_status) VALUES ($1, $1, 'memory', $2, 'desktop', $3)",
        )
        .bind(draft)
        .bind(json!({"create": {"path": "new.md", "content": content}}).to_string())
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn prune(pool: &SqlitePool, pins: &[&str]) {
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        prune_in_transaction(&mut tx, pins.iter().map(|id| (*id).to_owned()).collect())
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    async fn commit_ids(pool: &SqlitePool) -> Vec<String> {
        sqlx::query_scalar("SELECT commit_id FROM cached_commits ORDER BY commit_id")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn roots_keep_live_work_and_org_sources_without_retaining_terminal_history() {
        let pool = pool().await;
        for id in [
            "previous",
            "active_base",
            "active_current",
            "pending_base",
            "pending_current",
            "pinned",
            "nested_source",
            "operation_source",
            "retired",
            "old_parent",
            "body_mention",
        ] {
            cache_commit(&pool, id, None, None, "shared body").await;
        }
        cache_commit(
            &pool,
            "source",
            Some("old_parent"),
            Some("nested_source"),
            "source body",
        )
        .await;
        cache_commit(
            &pool,
            "current",
            Some("old_parent"),
            Some("source"),
            "current body",
        )
        .await;
        set_ref(&pool, "current").await;
        sqlx::query("INSERT INTO daemon_meta VALUES ('previous_project_generation:p', 'previous')")
            .execute(&pool)
            .await
            .unwrap();
        draft(&pool, "active", "active_base", "active_current", "open").await;
        operation(&pool, "active", "synced", Some("operation_source")).await;
        draft(
            &pool,
            "pending",
            "pending_base",
            "pending_current",
            "discarded",
        )
        .await;
        operation(&pool, "pending", "queued", None).await;
        draft(&pool, "terminal", "retired", "retired", "merged").await;
        operation(&pool, "terminal", "synced", Some("body_mention")).await;

        prune(&pool, &["pinned", "uncached_pin"]).await;
        assert_eq!(
            commit_ids(&pool).await,
            [
                "active_base",
                "active_current",
                "current",
                "nested_source",
                "operation_source",
                "pending_base",
                "pending_current",
                "pinned",
                "previous",
                "source"
            ]
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM cached_trees")
                .fetch_one(&pool)
                .await
                .unwrap(),
            10
        );
        // Shared Blob bytes stay deduplicated, including the selection Blob.
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM cached_blobs")
                .fetch_one(&pool)
                .await
                .unwrap(),
            4
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM local_drafts")
                .fetch_one(&pool)
                .await
                .unwrap(),
            3
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM local_draft_operations")
                .fetch_one(&pool)
                .await
                .unwrap(),
            3
        );

        sqlx::query(
            "UPDATE local_draft_operations SET sync_status = 'synced' WHERE draft_id = 'pending'",
        )
        .execute(&pool)
        .await
        .unwrap();
        prune(&pool, &[]).await;
        assert_eq!(
            commit_ids(&pool).await,
            [
                "active_base",
                "active_current",
                "current",
                "nested_source",
                "operation_source",
                "previous",
                "source"
            ]
        );
    }

    #[tokio::test]
    async fn unreadable_retained_dependencies_roll_back_the_entire_collection() {
        for damage in ["commit", "tree", "source", "blob", "operation"] {
            let pool = pool().await;
            let blob = cache_commit(&pool, "current", None, None, "current body").await;
            cache_commit(&pool, "unreachable", None, None, "retired body").await;
            set_ref(&pool, "current").await;
            match damage {
                "commit" => {
                    sqlx::query(
                        "UPDATE cached_commits SET payload_json = '{}' WHERE commit_id = 'current'",
                    )
                    .execute(&pool)
                    .await
                    .unwrap();
                }
                "tree" => {
                    sqlx::query("DELETE FROM cached_trees WHERE tree_id = 'tree_current'")
                        .execute(&pool)
                        .await
                        .unwrap();
                }
                "source" => {
                    sqlx::query("UPDATE cached_trees SET payload_json = json_set(payload_json, '$.entries[0].org_source', json('{\"resource_id\":\"mem_one\",\"commit_id\":42}')) WHERE tree_id = 'tree_current'")
                    .execute(&pool).await.unwrap();
                }
                "blob" => {
                    sqlx::query("DELETE FROM cached_blobs WHERE blob_id = $1")
                        .bind(&blob)
                        .execute(&pool)
                        .await
                        .unwrap();
                }
                "operation" => {
                    draft(&pool, "broken", "current", "current", "open").await;
                    operation(&pool, "broken", "queued", None).await;
                    sqlx::query("UPDATE local_draft_operations SET operation_json = '{}'")
                        .execute(&pool)
                        .await
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
            assert!(
                prune_in_transaction(&mut tx, vec![]).await.is_err(),
                "{damage}"
            );
            tx.rollback().await.unwrap();
            assert_eq!(
                commit_ids(&pool).await,
                ["current", "unreachable"],
                "{damage}"
            );
        }
    }

    #[tokio::test]
    async fn uncached_source_provenance_does_not_block_collection() {
        let pool = pool().await;
        cache_commit(&pool, "current", None, Some("server_only_source"), "keep").await;
        cache_commit(&pool, "retired", None, None, "old").await;
        set_ref(&pool, "current").await;
        prune(&pool, &[]).await;
        assert_eq!(commit_ids(&pool).await, ["current"]);
    }

    #[tokio::test]
    async fn collection_reclaims_database_pages_and_truncates_wal() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("local.db");
        let pool = migration::connect_local_db(&path).await.unwrap();
        migration::migrate_local_db(&pool).await.unwrap();
        migration::prepare_local_db_incremental_vacuum(&pool)
            .await
            .unwrap();
        cache_commit(&pool, "current", None, None, "keep").await;
        set_ref(&pool, "current").await;
        cache_commit(&pool, "retired", None, None, &"x".repeat(2 * 1024 * 1024)).await;
        migration::reclaim_local_db_pages(&pool).await.unwrap();
        let before = std::fs::metadata(&path).unwrap().len();
        prune(&pool, &[]).await;
        migration::reclaim_local_db_pages(&pool).await.unwrap();
        let after = std::fs::metadata(&path).unwrap().len();
        assert!(
            before > after + 1024 * 1024,
            "before={before}, after={after}"
        );
        assert_eq!(commit_ids(&pool).await, ["current"]);
        assert_eq!(
            std::fs::metadata(path.with_extension("db-wal"))
                .unwrap()
                .len(),
            0
        );
        pool.close().await;
    }
}
