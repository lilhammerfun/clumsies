//! SQL queries and persistence for review resources.

use super::model::review_status;
use crate::app::auth::AuthPrincipal;
use crate::app::draft::dto::{DraftCoordination, DraftFreshness, DraftReconciliationStatus};
use crate::app::draft::model::aggregate_draft_coordination;
use crate::app::organization::dto::UserRef;
use crate::app::review::dto::{Review, ReviewComment, ReviewListResponse};
use crate::error::ServerError;
use crate::pagination::page_info;
use sqlx::{FromRow, PgPool, Postgres, Row, Transaction};
use std::collections::BTreeMap;

/// Lock the mutable review containing a proposal, including non-primary members.
///
/// # Errors
/// Propagates database failures; the enclosing proposal mutation owns the transaction.
pub(super) async fn lock_review_for_draft(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<Option<String>, ServerError> {
    Ok(sqlx::query_scalar(
        "SELECT r.review_id FROM reviews r JOIN review_drafts rd USING (review_id)
         WHERE rd.draft_id = $1 AND r.status != 'merged' FOR UPDATE OF r",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Find the surviving members in their original review order.
///
/// # Errors
/// Propagates database failures.
pub(super) async fn remaining_review_drafts(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
    discarded_id: &str,
) -> Result<Vec<String>, ServerError> {
    Ok(sqlx::query_scalar(
        "SELECT rd.draft_id FROM review_drafts rd JOIN drafts d USING (draft_id)
         WHERE rd.review_id = $1 AND rd.draft_id != $2 AND d.status != 'discarded'
         ORDER BY rd.ordinal",
    )
    .bind(review_id)
    .bind(discarded_id)
    .fetch_all(&mut **tx)
    .await?)
}

/// Persist a smaller proposal set, or retain the final discarded member as rejected history.
///
/// # Errors
/// Propagates database failures without committing the caller's discard transaction.
pub(super) async fn remove_review_member(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
    draft_id: &str,
    next_primary: Option<&str>,
    actor_user_id: &str,
) -> Result<(), ServerError> {
    if next_primary.is_some() {
        sqlx::query("DELETE FROM review_drafts WHERE review_id = $1 AND draft_id = $2")
            .bind(review_id)
            .bind(draft_id)
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query(
        "UPDATE reviews SET draft_id = COALESCE($2, draft_id),
         status = CASE WHEN $2 IS NULL THEN 'rejected'
                       WHEN status = 'approved' THEN 'open' ELSE status END,
         version = version + 1, approved_result_hash = NULL,
         decision_body = CASE WHEN $2 IS NULL THEN 'Draft discarded.' ELSE NULL END,
         decided_by_user_id = CASE WHEN $2 IS NULL THEN $3 ELSE NULL END,
         decided_at = CASE WHEN $2 IS NULL THEN now() ELSE NULL END, updated_at = now()
         WHERE review_id = $1",
    )
    .bind(review_id)
    .bind(next_primary)
    .bind(actor_user_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Read a review's proposal identities in submission order.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn load_review_draft_ids(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
) -> Result<Vec<String>, ServerError> {
    let mut draft_ids = sqlx::query_scalar::<_, String>(
        "SELECT draft_id FROM review_drafts WHERE review_id = $1 ORDER BY ordinal",
    )
    .bind(review_id)
    .fetch_all(&mut **tx)
    .await?;
    if draft_ids.is_empty() {
        let primary =
            sqlx::query_scalar::<_, String>("SELECT draft_id FROM reviews WHERE review_id = $1")
                .bind(review_id)
                .fetch_optional(&mut **tx)
                .await?
                .ok_or_else(|| ServerError::not_found("review", review_id))?;
        draft_ids.push(primary);
    }
    Ok(draft_ids)
}

/// Batch-load proposal membership and freshness for a page of reviews.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and rejects inconsistent persisted state
/// or resource selections.
pub(crate) async fn load_review_list_projections(
    tx: &mut Transaction<'_, Postgres>,
    review_ids: &[String],
) -> Result<BTreeMap<String, (Vec<String>, Vec<DraftCoordination>)>, ServerError> {
    if review_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let rows = sqlx::query(
        "SELECT
            rd.review_id, rd.draft_id, d.base_commit_id,
            current_ref.commit_id AS current_commit_id,
            candidate.status AS candidate_status,
            candidate.candidate_id,
            EXISTS (
                SELECT 1 FROM draft_rebases r
                JOIN draft_reconciliation_candidates c ON c.candidate_id = r.candidate_id
                WHERE r.draft_id = d.draft_id AND r.resulting_draft_version = d.version
                  AND c.status = 'clean'
                  AND d.base_commit_id IS NOT DISTINCT FROM current_ref.commit_id
            ) AS auto_rebased,
            CASE
                WHEN d.base_commit_id IS NOT DISTINCT FROM current_ref.commit_id THEN FALSE
                WHEN base_entry.item_id IS NULL AND current_entry.item_id IS NULL THEN FALSE
                WHEN base_entry.item_id IS NOT NULL
                  AND current_entry.item_id IS NOT NULL
                  AND base_entry.item_id = current_entry.item_id
                  AND base_entry.path IS NOT DISTINCT FROM current_entry.path
                  AND base_entry.blob_id = current_entry.blob_id THEN FALSE
                ELSE TRUE
            END AS has_upstream_resource_changes
         FROM review_drafts rd
         JOIN drafts d ON d.draft_id = rd.draft_id
         JOIN projects p ON p.project_id = d.project_id
         JOIN refs current_ref
           ON current_ref.ref_name = 'refs/heads/main'
          AND (
              (d.resource_scope = 'org'
               AND current_ref.scope = 'org'
               AND current_ref.org_id = p.org_id)
              OR
              (d.resource_scope = 'project'
               AND current_ref.scope = 'project'
               AND current_ref.project_id = d.project_id)
          )
         LEFT JOIN LATERAL (
             SELECT operation.action
             FROM draft_operations operation
             WHERE operation.draft_id = d.draft_id
             ORDER BY operation.ordinal
             LIMIT 1
         ) first_operation ON TRUE
         LEFT JOIN LATERAL (
             SELECT e.item_id, e.path, e.blob_id
             FROM commits c
             JOIN tree_entries e ON e.tree_id = c.tree_id
             WHERE c.commit_id = d.base_commit_id
               AND d.base_commit_id IS DISTINCT FROM current_ref.commit_id
               AND e.resource_kind = 'memory'
               AND e.scope = d.resource_scope
               AND (
                   (d.target_id IS NOT NULL AND e.item_id = d.target_id)
                   OR
                   (d.target_id IS NULL
                    AND first_operation.action IS DISTINCT FROM 'create'
                    AND e.path = d.path)
               )
             ORDER BY e.item_id
             LIMIT 1
         ) base_entry ON TRUE
         LEFT JOIN LATERAL (
             SELECT e.item_id, e.path, e.blob_id
             FROM commits c
             JOIN tree_entries e ON e.tree_id = c.tree_id
             WHERE c.commit_id = current_ref.commit_id
               AND d.base_commit_id IS DISTINCT FROM current_ref.commit_id
               AND e.resource_kind = 'memory'
               AND e.scope = d.resource_scope
               AND (
                   (d.target_id IS NOT NULL AND e.item_id = d.target_id)
                   OR
                   (d.target_id IS NULL
                    AND first_operation.action IS DISTINCT FROM 'create'
                    AND e.path = d.path)
               )
             ORDER BY e.item_id
             LIMIT 1
         ) current_entry ON TRUE
         LEFT JOIN LATERAL (
             SELECT c.status, c.candidate_id
             FROM draft_reconciliation_candidates c
             WHERE c.draft_id = d.draft_id
               AND c.draft_version = d.version
               AND c.base_commit_id IS NOT DISTINCT FROM d.base_commit_id
               AND c.current_commit_id IS NOT DISTINCT FROM current_ref.commit_id
               AND c.invalidated_at IS NULL
               AND d.base_commit_id IS DISTINCT FROM current_ref.commit_id
             ORDER BY c.created_at DESC
             LIMIT 1
         ) candidate ON TRUE
         WHERE rd.review_id = ANY($1)
         ORDER BY rd.review_id, rd.ordinal",
    )
    .bind(review_ids)
    .fetch_all(&mut **tx)
    .await?;

    let mut projections = BTreeMap::<String, (Vec<String>, Vec<DraftCoordination>)>::new();
    for row in rows {
        let base_commit_id: Option<String> = row.try_get("base_commit_id")?;
        let current_commit_id: Option<String> = row.try_get("current_commit_id")?;
        let freshness = if base_commit_id == current_commit_id {
            DraftFreshness::Current
        } else {
            DraftFreshness::Behind
        };
        let candidate_status: Option<String> = row.try_get("candidate_status")?;
        let reconciliation = if freshness == DraftFreshness::Current {
            DraftReconciliationStatus::Unknown
        } else {
            match candidate_status.as_deref() {
                Some("clean") => DraftReconciliationStatus::Clean,
                Some("conflicts") => DraftReconciliationStatus::Conflicts,
                Some(status) => {
                    return Err(ServerError::InvalidRequest(format!(
                        "unknown reconciliation status: {status}"
                    )));
                }
                None => DraftReconciliationStatus::Unknown,
            }
        };
        let coordination = DraftCoordination {
            freshness,
            current_commit_id,
            has_upstream_resource_changes: row.try_get("has_upstream_resource_changes")?,
            reconciliation,
            candidate_id: row.try_get("candidate_id")?,
            auto_rebased: row.try_get("auto_rebased")?,
        };
        let projection = projections.entry(row.try_get("review_id")?).or_default();
        projection.0.push(row.try_get("draft_id")?);
        projection.1.push(coordination);
    }
    Ok(projections)
}

/// Decode review lifecycle and identity fields using supplied proposal coordination.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) fn review_from_row(
    row: &sqlx::postgres::PgRow,
    draft_ids: Vec<String>,
    coordination: DraftCoordination,
) -> Result<Review, ServerError> {
    Ok(Review {
        review_id: row.try_get("review_id")?,
        project_id: row.try_get("project_id")?,
        draft_id: row.try_get("draft_id")?,
        draft_ids,
        author: UserRef::from_row(row)?,
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        status: review_status(row.try_get::<String, _>("status")?.as_str())?,
        version: row.try_get("version")?,
        decision_body: row.try_get("decision_body")?,
        approved_result_hash: row.try_get("approved_result_hash")?,
        decided_by: row
            .try_get::<Option<String>, _>("decision_user_id")?
            .map(|user_id| {
                Ok::<UserRef, sqlx::Error>(UserRef {
                    user_id,
                    email: row.try_get("decision_user_email")?,
                    display_name: row.try_get("decision_user_display_name")?,
                    avatar_url: row.try_get("decision_user_avatar_url")?,
                    role: row.try_get("decision_user_role")?,
                })
            })
            .transpose()?,
        decided_at: row.try_get("decided_at")?,
        coordination,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

/// Read discussion with public author identity in chronological order.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn load_review_comments(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
) -> Result<Vec<ReviewComment>, ServerError> {
    let rows = sqlx::query(
        "SELECT
            c.comment_id, c.review_id, c.body, c.anchor_path, c.anchor_line,
            c.review_version, c.created_at,
            u.user_id, u.email, u.display_name, u.avatar_url, u.role
         FROM review_comments c
         JOIN users u ON u.user_id = c.author_user_id
         WHERE c.review_id = $1
         ORDER BY c.created_at, c.comment_id
         LIMIT 200",
    )
    .bind(review_id)
    .fetch_all(&mut **tx)
    .await?;

    rows.iter()
        .map(|row| {
            Ok(ReviewComment {
                comment_id: row.try_get("comment_id")?,
                review_id: row.try_get("review_id")?,
                author: UserRef::from_row(row)?,
                body: row.try_get("body")?,
                anchor_path: row.try_get("anchor_path")?,
                anchor_line: row.try_get("anchor_line")?,
                review_version: row.try_get("review_version")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect()
}

/// Check whether a review belongs to the principal's organization and an accessible project.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn review_is_accessible(
    pool: &PgPool,
    principal: &AuthPrincipal,
    review_id: &str,
) -> Result<bool, ServerError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1
            FROM reviews r
            JOIN projects p ON p.project_id = r.project_id
            JOIN project_members m ON m.project_id = p.project_id
            WHERE r.review_id = $1 AND p.org_id = $2 AND m.user_id = $3
         )",
    )
    .bind(review_id)
    .bind(&principal.org_id)
    .bind(&principal.user_id)
    .fetch_one(pool)
    .await?)
}

/// Find a rejected review previously associated with the primary proposal.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(crate) async fn find_rejected_review(
    pool: &PgPool,
    draft_id: &str,
) -> Result<Option<(String, i64)>, ServerError> {
    let row = sqlx::query(
        "SELECT review_id, version
         FROM reviews
         WHERE draft_id = $1 AND status = 'rejected'",
    )
    .bind(draft_id)
    .fetch_optional(pool)
    .await?;
    row.map(|row| Ok((row.try_get("review_id")?, row.try_get("version")?)))
        .transpose()
}

/// Require a review identity to exist before assembling its dependent records.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(crate) async fn ensure_review_exists(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
) -> Result<(), ServerError> {
    sqlx::query_scalar::<_, String>("SELECT review_id FROM reviews WHERE review_id = $1")
        .bind(review_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ServerError::not_found("review", review_id))?;
    Ok(())
}

/// Read member-visible reviews and batch their proposal coordination projections.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and rejects inconsistent persisted state
/// or resource selections.
pub(crate) async fn list_reviews(
    tx: &mut Transaction<'_, Postgres>,
    principal: &AuthPrincipal,
    project_id: Option<&str>,
) -> Result<ReviewListResponse, ServerError> {
    let rows = if let Some(project_id) = project_id {
        sqlx::query(
            "SELECT
                r.review_id, r.project_id, r.draft_id, r.title, r.description,
                r.status, r.version, r.decision_body, r.approved_result_hash,
                r.decided_at, r.created_at, r.updated_at,
                u.user_id, u.email, u.display_name, u.avatar_url, u.role,
                du.user_id AS decision_user_id, du.email AS decision_user_email,
                du.display_name AS decision_user_display_name,
                du.avatar_url AS decision_user_avatar_url, du.role AS decision_user_role
             FROM reviews r
             JOIN users u ON u.user_id = r.author_user_id
             LEFT JOIN users du ON du.user_id = r.decided_by_user_id
             JOIN projects p ON p.project_id = r.project_id
             JOIN project_members m ON m.project_id = p.project_id
             WHERE r.project_id = $1 AND p.org_id = $2 AND m.user_id = $3
             ORDER BY r.updated_at DESC, r.review_id
             LIMIT 200",
        )
        .bind(project_id)
        .bind(&principal.org_id)
        .bind(&principal.user_id)
        .fetch_all(&mut **tx)
        .await?
    } else {
        sqlx::query(
            "SELECT
                r.review_id, r.project_id, r.draft_id, r.title, r.description,
                r.status, r.version, r.decision_body, r.approved_result_hash,
                r.decided_at, r.created_at, r.updated_at,
                u.user_id, u.email, u.display_name, u.avatar_url, u.role,
                du.user_id AS decision_user_id, du.email AS decision_user_email,
                du.display_name AS decision_user_display_name,
                du.avatar_url AS decision_user_avatar_url, du.role AS decision_user_role
             FROM reviews r
             JOIN users u ON u.user_id = r.author_user_id
             LEFT JOIN users du ON du.user_id = r.decided_by_user_id
             JOIN projects p ON p.project_id = r.project_id
             JOIN project_members m ON m.project_id = p.project_id
             WHERE p.org_id = $1 AND m.user_id = $2
             ORDER BY r.updated_at DESC, r.review_id
             LIMIT 200",
        )
        .bind(&principal.org_id)
        .bind(&principal.user_id)
        .fetch_all(&mut **tx)
        .await?
    };
    let review_ids = rows
        .iter()
        .map(|row| row.try_get::<String, _>("review_id"))
        .collect::<Result<Vec<_>, _>>()?;
    let mut projections = load_review_list_projections(tx, &review_ids).await?;
    let mut items = Vec::with_capacity(review_ids.len());
    for row in rows {
        let review_id: String = row.try_get("review_id")?;
        let (draft_ids, coordinations) = projections.remove(&review_id).ok_or_else(|| {
            ServerError::InvalidRequest(format!("review {review_id} has no drafts"))
        })?;
        items.push(review_from_row(
            &row,
            draft_ids,
            aggregate_draft_coordination(&coordinations),
        )?);
    }
    Ok(ReviewListResponse {
        items,
        page_info: page_info(),
    })
}

/// Lock the current approved content fingerprint, distinguishing no approval from an absent hash.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn lock_approved_hash(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<Option<Option<String>>, ServerError> {
    Ok(sqlx::query_scalar::<_, Option<String>>(
        "SELECT approved_result_hash FROM reviews
         WHERE draft_id = $1 AND status = 'approved' FOR UPDATE",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Preserve or clear approval according to the service's content comparison and advance the
/// review revision.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn update_approval_after_content_change(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
    preserve_approval: bool,
) -> Result<(), ServerError> {
    sqlx::query(
        "UPDATE reviews
         SET status = CASE WHEN status = 'approved' AND NOT $2 THEN 'open' ELSE status END,
             approved_result_hash = CASE WHEN status = 'approved' AND $2 THEN approved_result_hash ELSE NULL END,
             decision_body = CASE WHEN status = 'approved' AND $2 THEN decision_body ELSE NULL END,
             decided_by_user_id = CASE WHEN status = 'approved' AND $2 THEN decided_by_user_id ELSE NULL END,
             decided_at = CASE WHEN status = 'approved' AND $2 THEN decided_at ELSE NULL END,
             version = version + 1,
             updated_at = now()
         WHERE draft_id = $1 AND status IN ('open', 'approved')",
    )
    .bind(draft_id)
    .bind(preserve_approval)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Lock the review revision against concurrent changes while a comment is appended.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn lock_comment_version(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
) -> Result<Option<ReviewVersion>, ServerError> {
    Ok(sqlx::query_as::<_, ReviewVersion>(
        "SELECT draft_id, version
             FROM reviews
             WHERE review_id = $1
             FOR UPDATE",
    )
    .bind(review_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Persist discussion with its validated final-content anchor and review revision.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn insert_comment(
    tx: &mut Transaction<'_, Postgres>,
    input: NewReviewComment<'_>,
) -> Result<(), ServerError> {
    sqlx::query(
        "INSERT INTO review_comments (
                comment_id, review_id, author_user_id, body, anchor_path, anchor_line,
                review_version
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(input.comment_id)
    .bind(input.review_id)
    .bind(input.author_user_id)
    .bind(input.body)
    .bind(input.anchor_path)
    .bind(input.anchor_line)
    .bind(input.review_version)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Lock the review lifecycle and revision before an approval or rejection.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn lock_decision_state(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
) -> Result<Option<ReviewDecisionState>, ServerError> {
    Ok(sqlx::query_as::<_, ReviewDecisionState>(
        "SELECT r.status AS review_status, r.version AS review_version
             FROM reviews r
             WHERE r.review_id = $1
             FOR UPDATE OF r",
    )
    .bind(review_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Lock a required proposal and read its current lifecycle for a review decision.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn lock_draft_status(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<String, ServerError> {
    Ok(
        sqlx::query_scalar("SELECT status FROM drafts WHERE draft_id = $1 FOR UPDATE")
            .bind(draft_id)
            .fetch_one(&mut **tx)
            .await?,
    )
}

/// Reopen a submitted proposal and return its new revision for event persistence.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn reopen_draft(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<DraftEventState, ServerError> {
    Ok(sqlx::query_as::<_, DraftEventState>(
        "UPDATE drafts
                     SET status = 'open', version = version + 1, updated_at = now()
                     WHERE draft_id = $1
                     RETURNING project_id, version",
    )
    .bind(draft_id)
    .fetch_one(&mut **tx)
    .await?)
}

/// Persist the selected review decision, actor, and content fingerprint at the next revision.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn update_decision(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
    status: &str,
    body: &Option<String>,
    approved_result_hash: &Option<String>,
    decided_by_user_id: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "UPDATE reviews
             SET status = $2, version = version + 1, decision_body = $3,
                 approved_result_hash = $4, decided_by_user_id = $5,
                 decided_at = now(), updated_at = now()
             WHERE review_id = $1",
    )
    .bind(review_id)
    .bind(status)
    .bind(body)
    .bind(approved_result_hash)
    .bind(decided_by_user_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Lock a proposal's ancestor snapshot while validating review reconciliation.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn lock_draft_base(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<Option<DraftBase>, ServerError> {
    Ok(sqlx::query_as::<_, DraftBase>(
        "SELECT base_commit_id FROM drafts WHERE draft_id = $1 FOR UPDATE",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Lock the primary proposal's ownership, metadata, and lifecycle for review creation.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn lock_primary_draft(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<Option<DraftReviewState>, ServerError> {
    Ok(sqlx::query_as::<_, DraftReviewState>(
        "SELECT draft_id, project_id, author_user_id, title, description, status, version,
                    base_commit_id, resource_scope
             FROM drafts
             WHERE draft_id = $1
             FOR UPDATE",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Lock an optional additional proposal's author, scope, and lifecycle for review creation.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn lock_additional_draft(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<Option<AdditionalDraftState>, ServerError> {
    Ok(sqlx::query_as::<_, AdditionalDraftState>(
        "SELECT project_id, author_user_id, status, version, base_commit_id, resource_scope
                 FROM drafts WHERE draft_id = $1 FOR UPDATE",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Mark a proposal submitted and return its new revision for synchronization events.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn submit_draft(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<DraftEventState, ServerError> {
    Ok(sqlx::query_as::<_, DraftEventState>(
        "UPDATE drafts
             SET status = 'submitted', version = version + 1, updated_at = now()
             WHERE draft_id = $1
             RETURNING project_id, version",
    )
    .bind(draft_id)
    .fetch_one(&mut **tx)
    .await?)
}

/// Persist initial open review metadata for a validated proposal set.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn insert_review(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
    draft_id: &str,
    project_id: &str,
    author_user_id: &str,
    title: &str,
    description: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "INSERT INTO reviews (
                review_id, draft_id, project_id, author_user_id, title, description,
                status, version
             )
             VALUES ($1, $2, $3, $4, $5, $6, 'open', 1)",
    )
    .bind(review_id)
    .bind(draft_id)
    .bind(project_id)
    .bind(author_user_id)
    .bind(title)
    .bind(description)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Persist one proposal's ordinal within a review's stable submission order.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn insert_review_draft(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
    draft_id: &str,
    ordinal: i32,
) -> Result<(), ServerError> {
    sqlx::query(
        "INSERT INTO review_drafts (review_id, draft_id, ordinal)
                 VALUES ($1, $2, $3)",
    )
    .bind(review_id)
    .bind(draft_id)
    .bind(ordinal)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Lock a review and its primary proposal together before resubmission.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn lock_submission_state(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
) -> Result<Option<ReviewSubmissionState>, ServerError> {
    Ok(sqlx::query_as::<_, ReviewSubmissionState>(
        "SELECT r.draft_id, r.status AS review_status, r.version AS review_version,
                    d.project_id, d.author_user_id, d.status AS draft_status,
                    d.version AS draft_version, d.base_commit_id, d.resource_scope
             FROM reviews r
             JOIN drafts d ON d.draft_id = r.draft_id
             WHERE r.review_id = $1
             FOR UPDATE OF r, d",
    )
    .bind(review_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Read the review currently linked to a proposal, if any.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn find_draft_review(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<Option<String>, ServerError> {
    Ok(
        sqlx::query_scalar("SELECT review_id FROM review_drafts WHERE draft_id = $1")
            .bind(draft_id)
            .fetch_optional(&mut **tx)
            .await?,
    )
}

/// Lock a required additional proposal before replacing a review's submission set.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn lock_required_additional_draft(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<AdditionalDraftState, ServerError> {
    Ok(sqlx::query_as::<_, AdditionalDraftState>(
        "SELECT project_id, author_user_id, status, version, base_commit_id, resource_scope
                 FROM drafts WHERE draft_id = $1 FOR UPDATE",
    )
    .bind(draft_id)
    .fetch_one(&mut **tx)
    .await?)
}

/// Advance the primary proposal to submitted and return its new revision.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn resubmit_primary_draft(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<i64, ServerError> {
    Ok(sqlx::query_scalar(
        "UPDATE drafts
             SET status = 'submitted', version = version + 1, updated_at = now()
             WHERE draft_id = $1
             RETURNING version",
    )
    .bind(draft_id)
    .fetch_one(&mut **tx)
    .await?)
}

/// Clear obsolete decision state, apply supplied metadata, and reopen the review.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn reopen_review(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
    title: Option<String>,
    description: Option<String>,
) -> Result<(), ServerError> {
    sqlx::query(
        "UPDATE reviews
             SET status = 'open', version = version + 1, decision_body = NULL,
                 approved_result_hash = NULL,
                 decided_by_user_id = NULL, decided_at = NULL,
                 title = COALESCE($2, title), description = COALESCE($3, description),
                 updated_at = now()
             WHERE review_id = $1",
    )
    .bind(review_id)
    .bind(title)
    .bind(description)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Remove prior ordered proposal links before replacing the submission set.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn delete_review_drafts(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
) -> Result<(), ServerError> {
    sqlx::query("DELETE FROM review_drafts WHERE review_id = $1")
        .bind(review_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Read the primary proposal's carrying project and scope for publication locking.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn load_coordination(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
) -> Result<Option<ReviewCoordination>, ServerError> {
    Ok(sqlx::query_as::<_, ReviewCoordination>(
        "SELECT draft.project_id, draft.resource_scope
             FROM reviews AS review
             JOIN drafts AS draft ON draft.draft_id = review.draft_id
             WHERE review.review_id = $1",
    )
    .bind(review_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Lock review lifecycle, revision, and approved fingerprint before publication.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn lock_merge_state(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
) -> Result<Option<ReviewMergeState>, ServerError> {
    Ok(sqlx::query_as::<_, ReviewMergeState>(
        "SELECT r.project_id, r.status, r.version, r.approved_result_hash
             FROM reviews r
             WHERE r.review_id = $1
             FOR UPDATE OF r",
    )
    .bind(review_id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Lock a reviewed proposal's ancestor, scope, lifecycle, and revision before publication.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn lock_merge_draft(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<DraftMergeState, ServerError> {
    Ok(sqlx::query_as::<_, DraftMergeState>(
        "SELECT resource_scope, base_commit_id, status, version
                     FROM drafts WHERE draft_id = $1 FOR UPDATE",
    )
    .bind(draft_id)
    .fetch_one(&mut **tx)
    .await?)
}

/// Persist merged review state, optionally recording approval performed by the same operation.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn mark_merged(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
    approve_open: bool,
    result_hash: &str,
    actor_user_id: &str,
) -> Result<(), ServerError> {
    sqlx::query(
        "UPDATE reviews
             SET status = 'merged', version = version + 1,
                 decision_body = CASE WHEN $2 THEN NULL ELSE decision_body END,
                 approved_result_hash = CASE WHEN $2 THEN $3 ELSE approved_result_hash END,
                 decided_by_user_id = CASE WHEN $2 THEN $4 ELSE decided_by_user_id END,
                 decided_at = CASE WHEN $2 THEN now() ELSE decided_at END,
                 updated_at = now()
             WHERE review_id = $1",
    )
    .bind(review_id)
    .bind(approve_open)
    .bind(result_hash)
    .bind(actor_user_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Record the snapshot and applied mutation count produced by review publication.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn insert_merge(
    tx: &mut Transaction<'_, Postgres>,
    merge_id: String,
    review_id: &str,
    commit_id: &str,
    operation_count: i32,
) -> Result<(), ServerError> {
    sqlx::query(
        "INSERT INTO review_merges (
                merge_id, review_id, commit_id, applied_operation_count
             )
             VALUES ($1, $2, $3, $4)",
    )
    .bind(merge_id)
    .bind(review_id)
    .bind(commit_id)
    .bind(operation_count)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Mark a proposal merged and return its new revision for event persistence.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn merge_draft(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<i64, ServerError> {
    Ok(sqlx::query_scalar(
        "UPDATE drafts
                 SET status = 'merged', version = version + 1, updated_at = now()
                 WHERE draft_id = $1
                 RETURNING version",
    )
    .bind(draft_id)
    .fetch_one(&mut **tx)
    .await?)
}

/// Decode persisted review metadata using supplied proposal identities and aggregate
/// coordination.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures and reports a missing required resource.
pub(super) async fn load_review(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
    draft_ids: Vec<String>,
    coordination: DraftCoordination,
) -> Result<Review, ServerError> {
    let row = sqlx::query(
        "SELECT
            r.review_id, r.project_id, r.draft_id, r.title, r.description,
            r.status, r.version, r.decision_body, r.approved_result_hash,
            r.decided_by_user_id, r.decided_at,
            r.created_at, r.updated_at,
            u.user_id, u.email, u.display_name, u.avatar_url, u.role,
            du.user_id AS decision_user_id, du.email AS decision_user_email,
            du.display_name AS decision_user_display_name,
            du.avatar_url AS decision_user_avatar_url, du.role AS decision_user_role
         FROM reviews r
         JOIN users u ON u.user_id = r.author_user_id
         LEFT JOIN users du ON du.user_id = r.decided_by_user_id
         WHERE r.review_id = $1",
    )
    .bind(review_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("review", review_id))?;
    review_from_row(&row, draft_ids, coordination)
}

/// Locked review revision against which a new discussion comment is anchored.
#[derive(sqlx::FromRow)]
pub(super) struct ReviewVersion {
    /// Monotonic revision or snapshot sequence used to detect concurrent changes.
    pub(super) version: i64,
}

/// Locked review lifecycle and revision against which an approval or rejection is checked.
#[derive(sqlx::FromRow)]
pub(super) struct ReviewDecisionState {
    /// Persisted review lifecycle state used for transition checks.
    pub(super) review_status: String,
    /// Review revision observed when the action or comment was created.
    pub(super) review_version: i64,
}

/// Project identity and new proposal revision needed to persist a lifecycle event.
#[derive(sqlx::FromRow)]
pub(super) struct DraftEventState {
    /// Project boundary containing the resource or proposal.
    pub(super) project_id: String,
    /// Monotonic revision or snapshot sequence used to detect concurrent changes.
    pub(super) version: i64,
}

/// Locked ancestor identity used when checking proposal reconciliation.
#[derive(sqlx::FromRow)]
pub(super) struct DraftBase {
    /// Commit against which the proposal was authored; absent before the first commit.
    pub(super) base_commit_id: Option<String>,
}

/// Locked primary proposal metadata, author, scope, and lifecycle for review creation.
#[derive(sqlx::FromRow)]
pub(super) struct DraftReviewState {
    /// Project boundary containing the resource or proposal.
    pub(super) project_id: String,
    /// Persisted identity of the author whose ownership is checked by the use case.
    pub(super) author_user_id: String,
    /// Human-readable summary of a proposal or review.
    pub(super) title: String,
    /// Human-readable explanation associated with the resource.
    pub(super) description: String,
    /// Proposal lifecycle controlling editing, review, and publication.
    pub(super) status: String,
    /// Monotonic revision or snapshot sequence used to detect concurrent changes.
    pub(super) version: i64,
    /// Commit against which the proposal was authored; absent before the first commit.
    pub(super) base_commit_id: Option<String>,
    /// Persisted organization or project ownership category.
    pub(super) resource_scope: String,
}

/// Locked ownership and lifecycle of an additional proposal entering review.
#[derive(sqlx::FromRow)]
pub(super) struct AdditionalDraftState {
    /// Project boundary containing the resource or proposal.
    pub(super) project_id: String,
    /// Persisted identity of the author whose ownership is checked by the use case.
    pub(super) author_user_id: String,
    /// Proposal lifecycle controlling editing, review, and publication.
    pub(super) status: String,
    /// Monotonic revision or snapshot sequence used to detect concurrent changes.
    pub(super) version: i64,
    /// Commit against which the proposal was authored; absent before the first commit.
    pub(super) base_commit_id: Option<String>,
    /// Persisted organization or project ownership category.
    pub(super) resource_scope: String,
}

/// Locked review and primary proposal state required to validate resubmission.
#[derive(sqlx::FromRow)]
pub(super) struct ReviewSubmissionState {
    /// Stable identifier of the editable proposal.
    pub(super) draft_id: String,
    /// Persisted review lifecycle state used for transition checks.
    pub(super) review_status: String,
    /// Review revision observed when the action or comment was created.
    pub(super) review_version: i64,
    /// Project boundary containing the resource or proposal.
    pub(super) project_id: String,
    /// Persisted identity of the author whose ownership is checked by the use case.
    pub(super) author_user_id: String,
    /// Persisted proposal lifecycle state used for transition checks.
    pub(super) draft_status: String,
    /// Proposal revision to which this record or candidate applies.
    pub(super) draft_version: i64,
    /// Commit against which the proposal was authored; absent before the first commit.
    pub(super) base_commit_id: Option<String>,
    /// Persisted organization or project ownership category.
    pub(super) resource_scope: String,
}

/// Carrying project and proposal scope used to acquire publication coordination locks.
#[derive(sqlx::FromRow)]
pub(super) struct ReviewCoordination {
    /// Project boundary containing the resource or proposal.
    pub(super) project_id: String,
    /// Persisted organization or project ownership category.
    pub(super) resource_scope: String,
}

/// Locked review revision and approved content fingerprint required for publication.
#[derive(sqlx::FromRow)]
pub(super) struct ReviewMergeState {
    /// Project boundary containing the resource or proposal.
    pub(super) project_id: String,
    /// Locked review lifecycle checked before publication.
    pub(super) status: String,
    /// Monotonic revision or snapshot sequence used to detect concurrent changes.
    pub(super) version: i64,
    /// Content fingerprint covered by the current review approval.
    pub(super) approved_result_hash: Option<String>,
}

/// Locked proposal lifecycle, revision, and ancestor needed for atomic publication.
#[derive(sqlx::FromRow)]
pub(super) struct DraftMergeState {
    /// Persisted organization or project ownership category.
    pub(super) resource_scope: String,
    /// Commit against which the proposal was authored; absent before the first commit.
    pub(super) base_commit_id: Option<String>,
    /// Proposal lifecycle controlling editing, review, and publication.
    pub(super) status: String,
    /// Monotonic revision or snapshot sequence used to detect concurrent changes.
    pub(super) version: i64,
}

/// Validated discussion content and optional anchor at a specific review revision.
pub(super) struct NewReviewComment<'a> {
    /// Stable identifier of a review comment.
    pub(super) comment_id: &'a str,
    /// Stable identifier of a review spanning one or more proposals.
    pub(super) review_id: &'a str,
    /// Persisted identity of the author whose ownership is checked by the use case.
    pub(super) author_user_id: &'a str,
    /// Stored text content, including Markdown where the resource contract permits it.
    pub(super) body: &'a str,
    /// Final reviewed resource path to which the comment belongs.
    pub(super) anchor_path: Option<&'a str>,
    /// One-based line in the final reviewed content, when the comment is anchored.
    pub(super) anchor_line: Option<i64>,
    /// Review revision observed when the action or comment was created.
    pub(super) review_version: i64,
}
