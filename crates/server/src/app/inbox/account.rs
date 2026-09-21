//! Personal onboarding and access events persisted with login or membership changes.

use sqlx::{Postgres, Transaction};

use crate::{app::auth::AuthPrincipal, identity::prefixed_id};

/// Original welcome content; the macOS catalog also supplies a Simplified Chinese translation.
const WELCOME_BODY: &str = "## Your projects\n\nProjects bring together the Memory your team uses. When someone adds you to a project, it appears in your project list.\n\n## Work with Memory\n\nOpen a project to browse its Memory. Your edits stay in Drafts until they are reviewed and published.\n\n## Stay informed\n\nInbox tells you about Reviews, remote updates to Memory your projects use, and changes to your access. Your everyday Draft edits do not create notifications.\n\nIf you do not have a project yet, ask a project administrator to add you.";

/// Create one welcome message when the user's first session is issued, before inserting it.
///
/// Uses the login transaction; existing users with sessions receive no retroactive welcome.
/// # Errors
/// Propagates database failures so login and the first welcome commit together.
pub(crate) async fn notify_welcome(
    tx: &mut Transaction<'_, Postgres>,
    user_id: &str,
    org_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO inbox_notifications
            (user_id, notification_id, org_id, kind, target_id, event_key, body)
         SELECT $1, 'welcome', $2, 'welcome', $1, 'welcome', $3
         WHERE NOT EXISTS (SELECT 1 FROM auth_sessions WHERE user_id = $1)
         ON CONFLICT (user_id, notification_id) DO NOTHING",
    )
    .bind(user_id)
    .bind(org_id)
    .bind(WELCOME_BODY)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Notify the affected user of an actual project membership or organization role change.
///
/// The caller must authorize and persist the change in this transaction. A missing prior role
/// means addition; a missing new role means removal. Neither a no-op nor the actor's own action
/// creates a notice. Each committed change has its own receipt and minimal project-name snapshot.
/// # Errors
/// Propagates persistence failures, rolling back the membership change with the caller.
pub(crate) async fn notify_access_change(
    tx: &mut Transaction<'_, Postgres>,
    principal: &AuthPrincipal,
    user_id: &str,
    project_id: Option<&str>,
    previous_role: Option<&str>,
    new_role: Option<&str>,
) -> Result<(), sqlx::Error> {
    if previous_role == new_role || principal.user_id == user_id {
        return Ok(());
    }
    let kind = match (project_id, previous_role, new_role) {
        (None, _, _) => "org_role_changed",
        (Some(_), None, _) => "project_joined",
        (Some(_), _, None) => "project_removed",
        (Some(_), _, Some(_)) => "project_role_changed",
    };
    let id = prefixed_id("notice");
    sqlx::query(
        "INSERT INTO inbox_notifications
            (user_id, notification_id, org_id, project_id, project_name_snapshot,
             kind, target_id, actor_user_id, event_key, previous_role, new_role)
         VALUES ($1, $2, $3, $4, (SELECT name FROM projects WHERE project_id = $4 AND org_id = $3),
                 $5, COALESCE($4, $3), $6, $2, $7, $8)",
    )
    .bind(user_id)
    .bind(id)
    .bind(&principal.org_id)
    .bind(project_id)
    .bind(kind)
    .bind(&principal.user_id)
    .bind(previous_role)
    .bind(new_role)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
