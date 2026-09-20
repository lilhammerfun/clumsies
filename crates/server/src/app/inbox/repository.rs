//! Transactional notification production and access-filtered personal receipts.
use sqlx::{PgPool, Postgres, Row, Transaction};

use super::dto::{InboxAction, InboxListResponse, InboxNotification, UpdateInboxRequest};
use crate::{app::auth::AuthPrincipal, error::ServerError};

/// Upserts a review notification for eligible recipients in the caller's transaction.
///
/// Review requests reach project-member administrators; discussion reaches the author and
/// participants and existing recipients, as do outcomes. The actor receives no new reminder.
/// # Errors
/// Any query failure aborts the source transaction, so business writes cannot lose their notice.
pub(crate) async fn notify_review(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
    actor_id: &str,
    kind: &str,
    event_key: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "INSERT INTO inbox_notifications
            (user_id, notification_id, project_id, kind, target_id, actor_user_id, event_key)
         SELECT m.user_id, 'review:' || r.review_id, r.project_id, $3, r.review_id, $2, $4
         FROM reviews r
         JOIN project_members m ON m.project_id = r.project_id
         JOIN users u ON u.user_id = m.user_id
         WHERE r.review_id = $1 AND m.user_id <> $2 AND u.status = 'active' AND (
             ($3 = 'review_requested' AND u.role IN ('owner', 'admin'))
             OR ($3 <> 'review_requested' AND (
                 m.user_id = r.author_user_id
                 OR EXISTS (SELECT 1 FROM review_comments c WHERE c.review_id = r.review_id AND c.author_user_id = m.user_id)
                 OR EXISTS (
                     SELECT 1 FROM inbox_notifications n WHERE n.user_id = m.user_id AND n.notification_id = 'review:' || r.review_id
                 )
             ))
         )
         ON CONFLICT (user_id, notification_id) DO UPDATE SET
             kind = EXCLUDED.kind, actor_user_id = EXCLUDED.actor_user_id,
             event_key = EXCLUDED.event_key, version = inbox_notifications.version + 1,
             occurred_at = clock_timestamp()
         WHERE inbox_notifications.event_key <> EXCLUDED.event_key",
    ).bind(review_id).bind(actor_id).bind(kind).bind(event_key).execute(&mut **tx).await?;
    Ok(())
}

/// Announces a changed selected-memory projection to the project's members.
///
/// # Errors
/// Fails with the enclosing publication transaction if notification persistence fails.
pub(crate) async fn notify_shared_update(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    commit_id: &str,
    actor_id: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "INSERT INTO inbox_notifications
            (user_id, notification_id, project_id, kind, target_id, actor_user_id, event_key)
         SELECT m.user_id, 'shared:' || m.project_id, m.project_id, 'shared_update', m.project_id, $3, $2
         FROM project_members m JOIN users u ON u.user_id = m.user_id
         WHERE m.project_id = $1 AND u.status = 'active' AND m.user_id <> $3
         ON CONFLICT (user_id, notification_id) DO UPDATE SET
             actor_user_id = EXCLUDED.actor_user_id, event_key = EXCLUDED.event_key,
             version = inbox_notifications.version + 1, occurred_at = clock_timestamp()
         WHERE inbox_notifications.event_key <> EXCLUDED.event_key",
    ).bind(project_id).bind(commit_id).bind(actor_id).execute(&mut **tx).await?;
    Ok(())
}

/// Reads a bounded keyset page, rechecking project membership and review existence.
///
/// # Errors
/// Invalid page sizes and database decoding failures are returned without partial pages.
pub(super) async fn list(
    pool: &PgPool,
    principal: &AuthPrincipal,
    cursor: Option<&str>,
    limit: i64,
) -> Result<InboxListResponse, ServerError> {
    let mut rows = sqlx::query(
        "SELECT n.*, p.name AS project_name, COALESCE(r.title, p.name) AS title,
                COALESCE(u.display_name, u.email) AS actor_name, r.status AS review_status,
                (((r.status = 'open' AND r.author_user_id <> $1) OR r.status = 'approved') AND $5 IN ('owner', 'admin')
                  OR (r.status = 'rejected' AND r.author_user_id = $1)) AS needs_action
         FROM inbox_notifications n
         JOIN projects p ON p.project_id = n.project_id
         JOIN project_members m ON m.project_id = p.project_id AND m.user_id = $1
         LEFT JOIN reviews r ON n.kind <> 'shared_update' AND r.review_id = n.target_id AND r.project_id = p.project_id
         LEFT JOIN users u ON u.user_id = n.actor_user_id
         WHERE n.user_id = $1 AND p.org_id = $2
           AND ($3::text IS NULL OR n.notification_id > $3)
           AND (n.kind = 'shared_update' OR r.review_id IS NOT NULL)
         ORDER BY n.notification_id LIMIT $4",
    ).bind(&principal.user_id).bind(&principal.org_id).bind(cursor).bind(limit + 1)
        .bind(&principal.role).fetch_all(pool).await?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let items = rows
        .iter()
        .map(|row| {
            Ok(InboxNotification {
                notification_id: row.try_get("notification_id")?,
                project_id: row.try_get("project_id")?,
                project_name: row.try_get("project_name")?,
                kind: row.try_get("kind")?,
                target_id: row.try_get("target_id")?,
                title: row.try_get("title")?,
                actor_name: row.try_get("actor_name")?,
                version: row.try_get("version")?,
                read_version: row.try_get("read_version")?,
                archived_version: row.try_get("archived_version")?,
                needs_action: row
                    .try_get::<Option<bool>, _>("needs_action")?
                    .unwrap_or(false),
                review_status: row.try_get("review_status")?,
                occurred_at: row.try_get("occurred_at")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let next_cursor = if has_more {
        items.last().map(|item| item.notification_id.clone())
    } else {
        None
    };
    Ok(InboxListResponse { items, next_cursor })
}

/// Changes only the authenticated user's displayed receipt version.
///
/// # Errors
/// Returns not found for missing or revoked subjects; invalid/future versions cannot write.
pub(super) async fn update(
    pool: &PgPool,
    principal: &AuthPrincipal,
    id: &str,
    request: UpdateInboxRequest,
) -> Result<(), ServerError> {
    let action = match request.action {
        InboxAction::Read => "read",
        InboxAction::Unread => "unread",
        InboxAction::Archive => "archive",
        InboxAction::Restore => "restore",
    };
    let result = sqlx::query(
        "UPDATE inbox_notifications n SET
             read_version = CASE WHEN $5 = 'read' THEN GREATEST(n.read_version, $4)
                                 WHEN $5 = 'unread' AND n.version = $4 THEN 0 ELSE n.read_version END,
             archived_version = CASE WHEN $5 = 'archive' THEN GREATEST(n.archived_version, $4)
                                     WHEN $5 = 'restore' AND n.archived_version <= $4 THEN 0 ELSE n.archived_version END
         FROM projects p, project_members m
         WHERE n.user_id = $1 AND n.notification_id = $3 AND n.version >= $4
           AND p.project_id = n.project_id AND p.org_id = $2
           AND m.project_id = p.project_id AND m.user_id = $1
           AND (n.kind = 'shared_update' OR EXISTS (
               SELECT 1 FROM reviews r WHERE r.review_id = n.target_id AND r.project_id = p.project_id
           ))",
    ).bind(&principal.user_id).bind(&principal.org_id).bind(id).bind(request.version)
        .bind(action).execute(pool).await?;
    if result.rows_affected() == 0 {
        return Err(ServerError::not_found("notification", id));
    }
    Ok(())
}
