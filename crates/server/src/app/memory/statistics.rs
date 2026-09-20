//! Read-only published snapshot statistics, with no blob downloads or persistent rollups.

use super::dto::{
    MemoryStatistics, MemoryStatisticsChange, MemoryStatisticsDay, MemoryStatisticsQuery,
    StatisticsResource,
};
use crate::app::auth::AuthPrincipal;
use crate::error::ServerError;
use sqlx::{PgPool, Row};
use std::collections::{BTreeMap, BTreeSet};
use time::OffsetDateTime;

/// Loads one consistent view of history and current inventory.
///
/// # Errors
/// Rejects unknown time zones and propagates database/decoding failures.
pub(super) async fn load(
    pool: &PgPool,
    principal: &AuthPrincipal,
    project_id: Option<&str>,
    query: MemoryStatisticsQuery,
) -> Result<MemoryStatistics, ServerError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let valid: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_timezone_names WHERE name = $1)")
            .bind(&query.time_zone)
            .fetch_one(&mut *tx)
            .await?;
    if !valid {
        return Err(ServerError::InvalidRequest("Unknown IANA time zone".into()));
    }
    let now = OffsetDateTime::now_utc();
    let bounds = calendar_bounds(&mut tx, now, &query.time_zone).await?;
    let day_bounds = bounds[90 - query.days as usize..].to_vec();
    let start = day_bounds[0];
    let project_ids: Vec<String> = sqlx::query_scalar(
        "SELECT p.project_id FROM projects p JOIN project_members m USING(project_id)
         WHERE p.org_id = $1 AND m.user_id = $2 AND ($3::text IS NULL OR p.project_id = $3) ORDER BY p.project_id"
    ).bind(&principal.org_id).bind(&principal.user_id).bind(project_id).fetch_all(&mut *tx).await?;
    if let Some(project_id) = project_id
        && !project_ids.iter().any(|id| id == project_id)
    {
        return Err(ServerError::not_found("project", project_id));
    }
    let drafts = sqlx::query(
        "SELECT count(*) FILTER (WHERE status IN ('open','conflicted')) AS open,
                count(*) FILTER (WHERE status = 'submitted') AS submitted
         FROM drafts WHERE project_id = ANY($1)",
    )
    .bind(&project_ids)
    .fetch_one(&mut *tx)
    .await?;
    // One baseline before the requested window is sufficient. Only metadata is read.
    let rows = sqlx::query(
        "WITH RECURSIVE history AS (
           SELECT c.* FROM commits c JOIN refs r ON r.commit_id = c.commit_id
           WHERE r.org_id = $1 AND r.ref_name = 'refs/heads/main'
             AND r.scope = CASE WHEN $2::text IS NULL THEN 'org' ELSE 'project' END
             AND r.project_id IS NOT DISTINCT FROM $2
           UNION
           SELECT p.* FROM commits p JOIN history c ON p.commit_id = c.parent_commit_id
           WHERE c.created_at >= to_timestamp($3) AND p.org_id = $1
             AND p.project_id IS NOT DISTINCT FROM $2
         )
         SELECT h.version, h.created_at, h.parent_commit_id, e.item_id, e.path, e.blob_id,
                e.description, COALESCE(r.name, e.path, e.item_id) AS title
         FROM history h LEFT JOIN tree_entries e ON e.tree_id = h.tree_id AND e.resource_kind = 'memory'
         LEFT JOIN resources r ON r.resource_id = e.item_id
         ORDER BY h.version, e.item_id"
    ).bind(&principal.org_id).bind(project_id).bind(start as f64).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    let mut versions: BTreeMap<i64, Snapshot> = BTreeMap::new();
    for row in rows {
        let at: OffsetDateTime = row.try_get("created_at")?;
        let parent: Option<String> = row.try_get("parent_commit_id")?;
        let snapshot = versions
            .entry(row.try_get("version")?)
            .or_insert_with(|| Snapshot {
                at: at.unix_timestamp(),
                has_parent: parent.is_some(),
                entries: BTreeMap::new(),
            });
        if let Some(id) = row.try_get::<Option<String>, _>("item_id")? {
            snapshot.entries.insert(
                id,
                Entry {
                    path: row.try_get("path")?,
                    blob: row.try_get("blob_id")?,
                    description: row.try_get("description")?,
                    title: row.try_get("title")?,
                },
            );
        }
    }
    let snapshots: Vec<Snapshot> = versions.into_values().collect();
    let mut events = Vec::new();
    let empty = BTreeMap::new();
    for (index, snapshot) in snapshots.iter().enumerate() {
        if snapshot.at < start || snapshot.at > now.unix_timestamp() {
            continue;
        }
        if index == 0 && snapshot.has_parent {
            continue;
        }
        let previous = if index == 0 {
            &empty
        } else {
            &snapshots[index - 1].entries
        };
        for (id, entry) in &snapshot.entries {
            let kind = match previous.get(id) {
                None => Some("added"),
                Some(old)
                    if old.path != entry.path
                        || old.blob != entry.blob
                        || old.description != entry.description =>
                {
                    Some("updated")
                }
                _ => None,
            };
            if let Some(kind) = kind {
                events.push((snapshot.at, id, kind));
            }
        }
        for id in previous
            .keys()
            .filter(|id| !snapshot.entries.contains_key(*id))
        {
            events.push((snapshot.at, id, "deleted"));
        }
    }
    let count = |kind: &str, from: i64, until: i64| {
        events
            .iter()
            .filter(|(at, _, k)| *at >= from && *at < until && *k == kind)
            .map(|(_, id, _)| id)
            .collect::<BTreeSet<_>>()
            .len()
    };
    let stride = if query.days == 7 { 1 } else { 7 };
    let mut change_buckets = Vec::new();
    for index in (0..query.days as usize).step_by(stride) {
        for kind in ["added", "updated", "deleted"] {
            change_buckets.push(MemoryStatisticsChange {
                date: day_bounds[index],
                kind: kind.into(),
                count: count(
                    kind,
                    day_bounds[index],
                    day_bounds[(index + stride).min(query.days as usize)],
                ),
            });
        }
    }
    let days = day_bounds
        .windows(2)
        .map(|day| MemoryStatisticsDay {
            date: day[0],
            memory_count: snapshots
                .iter()
                .rev()
                .find(|s| s.at < day[1] && s.at <= now.unix_timestamp())
                .map(|s| s.entries.len()),
        })
        .collect();
    let resources: Vec<_> = snapshots
        .last()
        .map(|s| {
            s.entries
                .iter()
                .map(|(id, e)| StatisticsResource {
                    id: id.clone(),
                    path: e.path.clone(),
                    title: e.title.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(MemoryStatistics {
        generated_at: now.unix_timestamp(),
        day_bounds,
        recency_starts: vec![bounds[83], bounds[60], bounds[0]],
        project_ids,
        memory_count: resources.len(),
        resources,
        added_count: count("added", start, now.unix_timestamp() + 1),
        updated_count: count("updated", start, now.unix_timestamp() + 1),
        deleted_count: count("deleted", start, now.unix_timestamp() + 1),
        days,
        change_buckets,
        open_drafts: drafts.try_get("open")?,
        submitted_drafts: drafts.try_get("submitted")?,
    })
}

/// A commit's metadata-only inventory.
struct Snapshot {
    /// Publication time in Unix seconds.
    at: i64,
    /// Whether this snapshot has a predecessor.
    has_parent: bool,
    /// Stable document IDs mapped to content identity and path.
    entries: BTreeMap<String, Entry>,
}

/// Metadata used to compare two published entries without loading their bodies.
struct Entry {
    /// Published path.
    path: String,
    /// Content identity.
    blob: String,
    /// Retrieval description; changing it is also an update.
    description: String,
    /// Display title from current resource metadata.
    title: String,
}

/// Computes civil-day boundaries in PostgreSQL so historical DST rules are preserved.
///
/// # Errors
/// Propagates database failures or an unsupported time zone.
async fn calendar_bounds(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    now: OffsetDateTime,
    zone: &str,
) -> Result<Vec<i64>, ServerError> {
    Ok(sqlx::query_scalar(
        "SELECT extract(epoch FROM ((date_trunc('day', $1::timestamptz AT TIME ZONE $2) + n * interval '1 day') AT TIME ZONE $2))::bigint
         FROM generate_series(-89, 1) n ORDER BY n"
    ).bind(now).bind(zone).fetch_all(&mut **tx).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use testcontainers::runners::AsyncRunner;
    use testcontainers_modules::postgres::Postgres;

    #[tokio::test]
    async fn calendar_days_preserve_dst_midnights() {
        let container = Postgres::default().start().await.unwrap();
        let port = container.get_host_port_ipv4(5432).await.unwrap();
        let pool = PgPool::connect(&format!(
            "postgres://postgres:postgres@127.0.0.1:{port}/postgres"
        ))
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        let bounds = calendar_bounds(
            &mut tx,
            time::macros::datetime!(2026-03-10 12:00 UTC),
            "America/Los_Angeles",
        )
        .await
        .unwrap();
        assert_eq!(bounds.len(), 91);
        assert_eq!(
            bounds[83],
            time::macros::datetime!(2026-03-04 08:00 UTC).unix_timestamp()
        );
        assert_eq!(
            bounds[89],
            time::macros::datetime!(2026-03-10 07:00 UTC).unix_timestamp()
        );
        assert_eq!(
            bounds
                .windows(2)
                .filter(|w| w[1] - w[0] == 23 * 3600)
                .count(),
            1
        );
        tx.rollback().await.unwrap();
        pool.close().await;
    }
}
