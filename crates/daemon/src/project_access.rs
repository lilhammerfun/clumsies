//! Session-scoped project availability for synchronization and local recovery.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{DaemonError, DaemonProjectBinding, DaemonState, commit_sync, get_server_json};

/// A live membership snapshot, valid only for the session that fetched it.
#[derive(Clone)]
pub(crate) struct ProjectAccess {
    /// Session generation; token refresh does not change this identity.
    session_revision: u64,
    /// Organization whose reference can sync even when there are no projects.
    pub(crate) org_id: String,
    /// Projects explicitly accessible to the authenticated user.
    pub(crate) project_ids: BTreeSet<String>,
}

/// Locally retained work whose project is absent from the live membership list.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct UnavailableProject {
    /// Project identity retained for recovery, never reassigned by name.
    pub project_id: String,
    /// Local directory associations, including directories that no longer exist.
    pub bindings: Vec<DaemonProjectBinding>,
    /// Open or submitted drafts retained on this Mac.
    pub draft_count: i64,
}

/// The membership fields of the authenticated identity response.
#[derive(Deserialize)]
struct Identity {
    /// Authenticated organization.
    org: Organization,
    /// Complete, unpaginated project membership list.
    projects: Vec<Project>,
}

/// Organization identity from the server.
#[derive(Deserialize)]
struct Organization {
    /// Stable organization ID.
    org_id: String,
}

/// Project identity from the server.
#[derive(Deserialize)]
struct Project {
    /// Stable project ID.
    project_id: String,
}

/// Refresh membership directly from the server without using a fallback cache.
/// Failed reads leave the previous snapshot intact; another session cannot inherit it.
///
/// # Errors
/// Returns HTTP, decoding, validation, or session-change errors without changing local data.
pub(crate) async fn refresh(state: &DaemonState) -> Result<(), DaemonError> {
    let session = state.project_config_with_credentials_snapshot().await;
    let identity: Identity = get_server_json(state, "/api/v1/me").await?;
    commit_sync::validate_cache_component("org_id", &identity.org.org_id)?;
    for project in &identity.projects {
        commit_sync::validate_cache_component("project_id", &project.project_id)?;
    }
    if state.project_config_snapshot().session_revision != session.session_revision {
        return Err(session_changed());
    }
    *state
        .inner
        .project_access
        .write()
        .expect("project access lock poisoned") = Some(ProjectAccess {
        session_revision: session.session_revision,
        org_id: identity.org.org_id,
        project_ids: identity
            .projects
            .into_iter()
            .map(|p| p.project_id)
            .collect(),
    });
    state.reconcile_project_selection().await?;
    crate::draft::resume_deferred_events(state, &require_current(state)?.project_ids).await?;
    Ok(())
}

/// Return only membership observed in the current login session.
pub(crate) fn current(state: &DaemonState) -> Option<ProjectAccess> {
    let revision = state.project_config_snapshot().session_revision;
    state
        .inner
        .project_access
        .read()
        .expect("project access lock poisoned")
        .as_ref()
        .filter(|access| access.session_revision == revision)
        .cloned()
}

/// Require a live membership snapshot before issuing synchronization requests.
///
/// # Errors
/// Returns a session-change error when the current identity has not been checked.
pub(crate) fn require_current(state: &DaemonState) -> Result<ProjectAccess, DaemonError> {
    current(state).ok_or_else(session_changed)
}

/// Refuse a locally bound project after the server has withdrawn its availability.
///
/// # Errors
/// Returns a recoverable binding error without deleting cached data or drafts.
pub(crate) fn ensure_available(state: &DaemonState, project_id: &str) -> Result<(), DaemonError> {
    if current(state).is_some_and(|access| !access.project_ids.contains(project_id)) {
        return Err(DaemonError::State {
            code: "project_binding_unresolved",
            message: format!(
                "Project {project_id} was deleted or is no longer accessible. Local drafts are retained; manage its local bindings in Inbox."
            ),
        });
    }
    Ok(())
}

/// Encode membership for SQLite filters; absent knowledge does not hide local work.
pub(crate) fn project_ids_json(state: &DaemonState) -> Option<String> {
    current(state).map(|access| {
        serde_json::to_string(&access.project_ids).expect("string set is serializable")
    })
}

/// List retained associations and drafts independently of the remote project directory.
///
/// # Errors
/// Propagates local database errors; no records or files are removed.
pub(crate) async fn unavailable(
    state: &DaemonState,
) -> Result<Vec<UnavailableProject>, DaemonError> {
    let Some(access) = current(state) else {
        return Ok(Vec::new());
    };
    let candidates = commit_sync::sync_project_ids(state).await?;
    let mut projects = Vec::new();
    for project_id in candidates.difference(&access.project_ids) {
        let bindings = state
            .list_project_bindings(crate::DaemonProjectBindingListRequest {
                project_id: project_id.clone(),
            })
            .await?
            .items;
        let draft_count = sqlx::query("SELECT COUNT(*) AS count FROM local_drafts WHERE project_id = $1 AND status IN ('open', 'submitted')")
            .bind(project_id).fetch_one(&state.inner.pool).await?.try_get("count")?;
        projects.push(UnavailableProject {
            project_id: project_id.clone(),
            bindings,
            draft_count,
        });
    }
    Ok(projects)
}

/// Explain why an in-flight sync cannot continue under a different identity.
fn session_changed() -> DaemonError {
    DaemonError::State {
        code: "sync_session_changed",
        message: "The Server session changed; synchronize again with the current account."
            .to_owned(),
    }
}
