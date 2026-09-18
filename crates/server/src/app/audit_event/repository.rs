//! SQL queries and persistence for audit event resources.

use crate::app::audit_event::dto::AuditEvent;
use crate::error::ServerError;
use crate::identity::prefixed_id;
use sqlx::{PgPool, Postgres, Row, Transaction};

pub(crate) async fn list_audit_events(
    pool: &PgPool,
    org_id: &str,
    offset: i64,
    limit: i64,
    query: Option<&str>,
) -> Result<Vec<AuditEvent>, ServerError> {
    let rows = sqlx::query(
        "WITH events AS (
            SELECT e.event_id, e.actor_user_id, actor.display_name AS actor_display_name,
                   actor.email AS actor_email, e.action, e.target_type, e.target_id, e.created_at,
                   CASE e.target_type
                       WHEN 'org' THEN target_org.name
                       WHEN 'project' THEN target_project.name
                       WHEN 'user' THEN COALESCE(target_user.display_name, target_user.email)
                       WHEN 'project_member' THEN target_project.name || ' · ' ||
                           COALESCE(target_user.display_name, target_user.email)
                   END AS target_display_name
            FROM audit_events e
            LEFT JOIN users actor ON actor.user_id = e.actor_user_id
            LEFT JOIN orgs target_org ON e.target_type = 'org'
                AND target_org.org_id = e.target_id AND target_org.org_id = e.org_id
            LEFT JOIN projects target_project ON target_project.org_id = e.org_id
                AND target_project.project_id = CASE e.target_type
                    WHEN 'project' THEN e.target_id
                    WHEN 'project_member' THEN split_part(e.target_id, ':', 1)
                END
            LEFT JOIN users target_user ON target_user.user_id = CASE e.target_type
                WHEN 'user' THEN e.target_id
                WHEN 'project_member' THEN split_part(e.target_id, ':', 2)
            END
            WHERE e.org_id = $1
         )
         SELECT * FROM events
         WHERE $4::text IS NULL OR strpos(lower(concat_ws(' ', action,
             actor_display_name, actor_email, target_display_name)), lower($4)) > 0
         ORDER BY created_at DESC, event_id
         LIMIT $2 OFFSET $3",
    )
    .bind(org_id)
    .bind(limit)
    .bind(offset)
    .bind(query)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(AuditEvent {
                event_id: row.try_get("event_id")?,
                actor_user_id: row.try_get("actor_user_id")?,
                actor_display_name: row.try_get("actor_display_name")?,
                actor_email: row.try_get("actor_email")?,
                action: row.try_get("action")?,
                target_type: row.try_get("target_type")?,
                target_id: row.try_get("target_id")?,
                target_display_name: row.try_get("target_display_name")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect()
}

pub(crate) async fn insert_audit_event(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    actor_user_id: Option<&str>,
    action: &str,
    target_type: &str,
    target_id: Option<&str>,
) -> Result<(), ServerError> {
    sqlx::query(
        "INSERT INTO audit_events (
            event_id, org_id, actor_user_id, action, target_type, target_id
         ) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(prefixed_id("evt"))
    .bind(org_id)
    .bind(actor_user_id)
    .bind(action)
    .bind(target_type)
    .bind(target_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
