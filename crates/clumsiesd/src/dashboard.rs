//! Aggregate retained local retrieval telemetry without exporting trace payloads to the App.

use crate::{DaemonError, DaemonState};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::{BTreeMap, BTreeSet};

/// Scope and calendar boundaries supplied by the authorized server statistics response.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DashboardRetrievalRequest {
    /// Only projects visible in the active organization/scope.
    pub project_ids: Vec<String>,
    /// Published current inventory, for coverage and ranking denominators.
    pub resources: Vec<DashboardResource>,
    /// Calendar boundaries including the exclusive upper bound.
    pub day_bounds: Vec<i64>,
    /// Starts of the last 7, 30 and 90 calendar days.
    pub recency_starts: Vec<i64>,
    /// Server observation time as Unix seconds.
    pub generated_at: i64,
}

/// Current document metadata; no memory body or retrieval query is transmitted.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DashboardResource {
    /// Stable memory identity.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Published path.
    pub path: String,
}

/// Ready-to-render local telemetry statistics.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DashboardRetrievalStatistics {
    /// Completed retained requests in the chosen period.
    pub retrievals: usize,
    /// Distinct current documents retrieved in the chosen period.
    pub recalled_count: usize,
    /// Fraction of current inventory retrieved.
    pub coverage: f64,
    /// Daily request outcome counts; only dates with retained records are emitted.
    pub days: Vec<DashboardRetrievalDay>,
    /// Directory counts and coverage of the current inventory.
    pub directories: Vec<DashboardBar>,
    /// Six most frequently retrieved current documents.
    pub top_resources: Vec<DashboardBar>,
    /// Four mutually exclusive last-retrieval groups.
    pub recency: Vec<DashboardBar>,
    /// Oldest retained completed request within 90 days, when present.
    pub history_start: Option<i64>,
    /// Per-project history ceiling; counts are never advertised as complete usage.
    pub retention_per_project: i64,
}

/// One local calendar day's completed requests.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DashboardRetrievalDay {
    /// Local midnight as Unix seconds.
    pub date: i64,
    /// Successful requests with returned or reused fragments.
    pub returned: usize,
    /// Successful requests without content.
    pub empty: usize,
    /// Failed requests.
    pub failed: usize,
}

/// Bar values and optional inventory denominator.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DashboardBar {
    /// Stable document or group key.
    pub id: String,
    /// Display label.
    pub label: String,
    /// Retrieved document count, request count, or group size.
    pub value: usize,
    /// Total documents for coverage bars only.
    pub total: Option<usize>,
}

impl DaemonState {
    /// Returns aggregate telemetry for the supplied visible inventory.
    ///
    /// # Errors
    /// Rejects invalid scope/calendar inputs and propagates local database failures.
    pub async fn dashboard_retrieval_statistics(
        &self,
        request: DashboardRetrievalRequest,
    ) -> Result<DashboardRetrievalStatistics, DaemonError> {
        aggregate(&self.inner.pool, request).await
    }
}

/// Reads only request outcomes and distinct selected document IDs in a consistent transaction.
///
/// # Errors
/// Rejects malformed boundaries or duplicate inventory IDs; propagates SQLite failures.
async fn aggregate(
    pool: &SqlitePool,
    request: DashboardRetrievalRequest,
) -> Result<DashboardRetrievalStatistics, DaemonError> {
    let r = request;
    if !matches!(r.day_bounds.len(), 8 | 31 | 91)
        || r.recency_starts.len() != 3
        || r.project_ids.len() > 10_000
        || r.resources.len() > 100_000
        || r.day_bounds
            .windows(2)
            .any(|w| !(72_000..=100_800).contains(&w[1].saturating_sub(w[0])))
        || r.recency_starts.windows(2).any(|w| w[0] <= w[1])
        || r.generated_at
            < *r.day_bounds
                .get(r.day_bounds.len().saturating_sub(2))
                .unwrap_or(&i64::MAX)
        || r.generated_at >= *r.day_bounds.last().unwrap_or(&i64::MIN)
        || r.resources
            .iter()
            .map(|d| &d.id)
            .collect::<BTreeSet<_>>()
            .len()
            != r.resources.len()
    {
        return Err(DaemonError::InvalidRequest(
            "Invalid dashboard scope or calendar boundaries".into(),
        ));
    }
    let start = r.day_bounds[0];
    let quarter = r.recency_starts[2];
    if quarter > start
        || r.generated_at.saturating_sub(quarter) > 92 * 86400
        || r.recency_starts[0] > r.generated_at
    {
        return Err(DaemonError::InvalidRequest(
            "Invalid dashboard recency window".into(),
        ));
    }
    let projects = serde_json::to_string(&r.project_ids)?;
    let mut tx = pool.begin().await?;
    let runs = sqlx::query(
        "SELECT run_id, CAST(strftime('%s',created_at) AS INTEGER) AS at, status, returned_fragment_count
         FROM retrieval_runs WHERE project_id IN (SELECT value FROM json_each($1))
           AND status IN ('succeeded','failed')
           AND CAST(strftime('%s',created_at) AS INTEGER) BETWEEN $2 AND $3"
    ).bind(&projects).bind(quarter).bind(r.generated_at).fetch_all(&mut *tx).await?;
    let selected = sqlx::query(
        "SELECT DISTINCT c.run_id, c.resource_id, CAST(strftime('%s',r.created_at) AS INTEGER) AS at
         FROM retrieval_run_candidates c JOIN retrieval_runs r ON r.run_id = c.run_id
         WHERE r.project_id IN (SELECT value FROM json_each($1)) AND r.status = 'succeeded' AND c.selected = 1
           AND CAST(strftime('%s',r.created_at) AS INTEGER) BETWEEN $2 AND $3"
    ).bind(&projects).bind(quarter).bind(r.generated_at).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    let mut days = BTreeMap::new();
    let mut history_start: Option<i64> = None;
    let mut retrievals = 0;
    for run in runs {
        let at: i64 = run.try_get("at")?;
        history_start = Some(history_start.map_or(at, |old| old.min(at)));
        if at < start {
            continue;
        }
        let index = r.day_bounds.partition_point(|bound| *bound <= at) - 1;
        let date = r.day_bounds[index];
        let day = days.entry(date).or_insert(DashboardRetrievalDay {
            date,
            returned: 0,
            empty: 0,
            failed: 0,
        });
        if run.try_get::<String, _>("status")? == "failed" {
            day.failed += 1;
        } else if run.try_get::<i64, _>("returned_fragment_count")? == 0 {
            day.empty += 1;
        } else {
            day.returned += 1;
        }
        retrievals += 1;
    }
    let mut hits = BTreeMap::<String, usize>::new();
    let mut latest = BTreeMap::<String, i64>::new();
    for row in selected {
        let id: String = row.try_get("resource_id")?;
        let at: i64 = row.try_get("at")?;
        latest
            .entry(id.clone())
            .and_modify(|old| *old = (*old).max(at))
            .or_insert(at);
        if at >= start {
            *hits.entry(id).or_default() += 1;
        }
    }
    // Skip a shared parent so a project containing only skills/ still has useful groups.
    let mut shared_directory: Vec<_> = r
        .resources
        .first()
        .map(|resource| resource.path.split('/').collect())
        .unwrap_or_default();
    shared_directory.pop();
    for resource in r.resources.iter().skip(1) {
        let parent = resource
            .path
            .rsplit_once('/')
            .map_or("", |(parent, _)| parent);
        let shared = shared_directory
            .iter()
            .copied()
            .zip(parent.split('/'))
            .take_while(|(left, right)| left == right)
            .count();
        shared_directory.truncate(shared);
    }
    let mut directories = BTreeMap::new();
    let mut top_resources = Vec::new();
    let mut recent = [0; 4];
    let mut recalled_count = 0;
    for resource in &r.resources {
        let directory = resource
            .path
            .rsplit_once('/')
            .map_or("Root".into(), |(parent, _)| {
                let directory = parent
                    .split('/')
                    .take(shared_directory.len() + 1)
                    .collect::<Vec<_>>()
                    .join("/");
                format!("{directory}/")
            });
        let bar = directories
            .entry(directory.clone())
            .or_insert(DashboardBar {
                id: directory.clone(),
                label: directory,
                value: 0,
                total: Some(0),
            });
        *bar.total.as_mut().expect("directory bars have totals") += 1;
        let count = hits.get(&resource.id).copied().unwrap_or(0);
        if count > 0 {
            recalled_count += 1;
            bar.value += 1;
            top_resources.push(DashboardBar {
                id: resource.id.clone(),
                label: resource.title.clone(),
                value: count,
                total: None,
            });
        }
        let index = match latest.get(&resource.id) {
            Some(at) if *at >= r.recency_starts[0] => 0,
            Some(at) if *at >= r.recency_starts[1] => 1,
            Some(_) => 2,
            None => 3,
        };
        recent[index] += 1;
    }
    let mut directories: Vec<DashboardBar> = directories.into_values().collect();
    directories.sort_by(|a, b| b.total.cmp(&a.total).then(a.id.cmp(&b.id)));
    top_resources.sort_by(|a, b| b.value.cmp(&a.value).then(a.id.cmp(&b.id)));
    top_resources.truncate(6);
    let recency = ["Last 7 days", "8–30 days", "31–90 days", "Not observed"]
        .into_iter()
        .enumerate()
        .map(|(i, label)| DashboardBar {
            id: i.to_string(),
            label: label.into(),
            value: recent[i],
            total: None,
        })
        .collect();
    Ok(DashboardRetrievalStatistics {
        retrievals,
        recalled_count,
        coverage: if r.resources.is_empty() {
            0.0
        } else {
            recalled_count as f64 / r.resources.len() as f64
        },
        days: days.into_values().collect(),
        directories,
        top_resources,
        recency,
        history_start,
        retention_per_project: crate::retrieval_history::RETRIEVAL_RUN_RETENTION_PER_PROJECT,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn retained_statistics_deduplicate_fragments_and_exclude_other_projects() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::retrieval_history::migrate(&pool).await.unwrap();
        let today = 1_780_000_000 / 86400 * 86400;
        for (id, project, status, count, at) in [
            ("ok", "p", "succeeded", 2, today),
            ("reuse", "p", "succeeded", 1, today + 1),
            ("empty", "p", "succeeded", 0, today),
            ("failed", "p", "failed", 0, today),
            ("running", "p", "running", 0, today),
            ("other", "other", "succeeded", 1, today),
            ("old", "p", "succeeded", 1, today - 10 * 86400),
            ("future", "p", "succeeded", 1, today + 86400),
        ] {
            sqlx::query("INSERT INTO retrieval_runs (run_id, project_id, query, activation_state_fingerprint, status, returned_fragment_count, created_at) VALUES ($1,$2,'private query','state',$3,$4,strftime('%Y-%m-%dT%H:%M:%fZ',$5,'unixepoch'))")
                .bind(id).bind(project).bind(status).bind(count).bind(at).execute(&pool).await.unwrap();
        }
        for (run, unit, resource, selected) in [
            ("ok", "a1", "a", 1),
            ("ok", "a2", "a", 1),
            ("ok", "b1", "b", 0),
            ("reuse", "a1", "a", 1),
            ("other", "a1", "a", 1),
            ("old", "b1", "b", 1),
            ("failed", "c1", "c", 1),
        ] {
            sqlx::query("INSERT INTO retrieval_run_candidates (run_id,candidate_order,unit_key,resource_id,scope,kind,path,heading_path_json,locator_json,content_hash,resource_content_hash,token_count,evidence_excerpt,selected,exclusion_reason,delta_action) VALUES ($1,0,$2,$3,'org','memory','knowledge/a.md','[]','{}','hash','hash',1,'private excerpt',$4,'selected','reuse')")
                .bind(run).bind(unit).bind(resource).bind(selected).execute(&pool).await.unwrap();
        }
        let request = DashboardRetrievalRequest {
            project_ids: vec!["p".into()],
            resources: ["a", "b", "c"]
                .into_iter()
                .map(|id| DashboardResource {
                    id: id.into(),
                    title: id.into(),
                    path: format!("knowledge/{id}.md"),
                })
                .collect(),
            day_bounds: (-6..=1).map(|offset| today + offset * 86400).collect(),
            recency_starts: vec![today - 6 * 86400, today - 29 * 86400, today - 89 * 86400],
            generated_at: today + 3600,
        };
        let result = aggregate(&pool, request.clone()).await.unwrap();
        assert_eq!(result.retrievals, 4);
        assert_eq!(result.days.len(), 1);
        assert_eq!(
            (
                result.days[0].returned,
                result.days[0].empty,
                result.days[0].failed
            ),
            (2, 1, 1)
        );
        assert_eq!(result.recalled_count, 1);
        assert_eq!(result.top_resources[0].value, 2);
        assert_eq!(result.top_resources.len(), 1);
        assert_eq!(result.directories[0].total, Some(3));
        assert_eq!(result.directories[0].value, 1);
        assert_eq!(
            result.recency.iter().map(|b| b.value).collect::<Vec<_>>(),
            vec![1, 1, 0, 1]
        );
        assert_eq!(result.history_start, Some(today - 10 * 86400));
        let mut nested = request.clone();
        for (resource, path) in nested.resources.iter_mut().zip([
            "skills/coding/a.md",
            "skills/coding/b.md",
            "skills/rust/c.md",
        ]) {
            resource.path = path.into();
        }
        let result = aggregate(&pool, nested.clone()).await.unwrap();
        assert_eq!(
            result
                .directories
                .iter()
                .map(|bar| (bar.label.as_str(), bar.value, bar.total))
                .collect::<Vec<_>>(),
            vec![("skills/coding/", 1, Some(2)), ("skills/rust/", 0, Some(1))]
        );
        assert_eq!(result.recalled_count, 1);
        assert_eq!(result.coverage, 1.0 / 3.0);
        nested.resources[2].path = "README.md".into();
        let result = aggregate(&pool, nested).await.unwrap();
        assert_eq!(
            result
                .directories
                .iter()
                .map(|bar| (bar.label.as_str(), bar.total))
                .collect::<Vec<_>>(),
            vec![("skills/", Some(2)), ("Root", Some(1))]
        );
        let mut invalid = request.clone();
        invalid.day_bounds.swap(0, 1);
        assert!(matches!(
            aggregate(&pool, invalid).await,
            Err(DaemonError::InvalidRequest(_))
        ));
        let mut empty = request;
        empty.project_ids.clear();
        let result = aggregate(&pool, empty).await.unwrap();
        assert_eq!(result.retrievals, 0);
        assert_eq!(result.recalled_count, 0);
        assert_eq!(result.recency[3].value, 3);
        pool.close().await;
    }
}
