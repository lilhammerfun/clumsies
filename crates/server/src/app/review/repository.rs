//! SQL queries and persistence for review resources.

use super::model::{ReviewMergeData, review_comment_line_count, review_status};
use super::service::{
    load_review_detail, load_review_drafts, reconcile_review_draft, review_result_hash,
};
use crate::app::auth::AuthPrincipal;
use crate::app::commit::service::{
    advance_org_ref, advance_project_ref, create_org_commit, create_project_commit,
    current_org_ref, current_project_ref, lock_org_ref_for_project_projection,
};
use crate::app::draft::dto::{
    DraftCoordination, DraftEventType, DraftFreshness, DraftReconciliationStatus,
};
use crate::app::draft::model::{
    CommitOutcome, aggregate_draft_coordination, content_text, ensure_publishable_draft_scope,
    materialize_draft_operations,
};
use crate::app::draft::service::{
    apply_operation, create_reconciliation_candidate_in_tx, draft_result_hash, draft_result_state,
    insert_draft_event, invalidate_draft_candidates, load_draft_operations, target_ref_for_draft,
    user_ref, user_ref_from_row, validate_org_draft_operation_inputs_are_selected,
    validate_stored_org_draft_operations_are_selected,
};
use crate::app::memory::dto::ResourceScope;
use crate::app::memory::model::{OrgResourceImpact, resource_scope};
use crate::app::memory::service::{
    lock_org_draft_selection_coordination_for_project, project_org_id,
    refresh_projects_for_org_resource_changes, resolve_org_resource_impact,
    select_created_org_resources_for_project,
};
use crate::app::organization::dto::UserRef;
use crate::app::review::dto::{
    CreateReviewCommentRequest, CreateReviewDecisionRequest, CreateReviewMergeRequest,
    CreateReviewRequest, CreateReviewSubmissionRequest, Review, ReviewComment, ReviewDecision,
    ReviewDetail, ReviewDraftDetail, ReviewDraftRequest, ReviewListResponse,
};
use crate::error::ServerError;
use crate::identity::prefixed_id;
use crate::pagination::page_info;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) async fn refresh_review_after_draft_content_change(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<(), ServerError> {
    let approved_hash = sqlx::query_scalar::<_, Option<String>>(
        "SELECT approved_result_hash FROM reviews
         WHERE draft_id = $1 AND status = 'approved' FOR UPDATE",
    )
    .bind(draft_id)
    .fetch_optional(&mut **tx)
    .await?
    .flatten();
    let new_hash = if approved_hash.is_some() {
        Some(draft_result_hash(tx, draft_id).await?)
    } else {
        None
    };
    let preserve_approval = approved_hash.is_some() && approved_hash == new_hash;
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

pub(crate) async fn load_review_with_drafts(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
) -> Result<(Review, Vec<ReviewDraftDetail>), ServerError> {
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

    let draft_ids = load_review_draft_ids(tx, review_id).await?;
    let drafts = load_review_drafts(tx, &draft_ids).await?;
    let coordinations = drafts
        .iter()
        .map(|detail| detail.draft.coordination.clone())
        .collect::<Vec<_>>();
    let review = review_from_row(
        &row,
        draft_ids,
        aggregate_draft_coordination(&coordinations),
    )?;
    Ok((review, drafts))
}

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
        };
        let projection = projections.entry(row.try_get("review_id")?).or_default();
        projection.0.push(row.try_get("draft_id")?);
        projection.1.push(coordination);
    }
    Ok(projections)
}

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
        author: user_ref_from_row(row)?,
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
                author: user_ref_from_row(row)?,
                body: row.try_get("body")?,
                anchor_path: row.try_get("anchor_path")?,
                anchor_line: row.try_get("anchor_line")?,
                review_version: row.try_get("review_version")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect()
}

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

pub(crate) async fn create_review_comment(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
    author_user_id: &str,
    request: CreateReviewCommentRequest,
) -> Result<ReviewComment, ServerError> {
    if request.body.trim().is_empty() {
        return Err(ServerError::InvalidRequest(
            "review comment body must not be empty".to_owned(),
        ));
    }
    let anchor = match (request.anchor_path.as_deref(), request.anchor_line) {
        (None, None) => None,
        (Some(path), Some(line)) if !path.is_empty() && line > 0 => Some((path, line)),
        (Some(_), Some(_)) => {
            return Err(ServerError::InvalidRequest(
                "review comment anchor_path must not be empty and anchor_line must be positive"
                    .to_owned(),
            ));
        }
        _ => {
            return Err(ServerError::InvalidRequest(
                "review comment anchor_path and anchor_line must be provided together".to_owned(),
            ));
        }
    };
    let review_row = sqlx::query(
        "SELECT draft_id, version
             FROM reviews
             WHERE review_id = $1
             FOR UPDATE",
    )
    .bind(review_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("review", review_id))?;
    let review_version: i64 = review_row.try_get("version")?;
    if review_version != request.expected_review_version {
        return Err(ServerError::version_conflict(
            "review",
            request.expected_review_version,
            review_version,
        ));
    }
    if let Some((anchor_path, anchor_line)) = anchor {
        let draft_ids = load_review_draft_ids(tx, review_id).await?;
        let mut matching_state = None;
        for draft_id in draft_ids {
            let state = draft_result_state(tx, &draft_id).await?;
            if state.exists && state.resource.path.as_deref() == Some(anchor_path) {
                matching_state = Some(state);
                break;
            }
        }
        let Some(final_state) = matching_state else {
            return Err(ServerError::InvalidRequest(format!(
                "review comment anchor_path must match a final review path ({anchor_path})"
            )));
        };
        let line_count = final_state
            .content
            .as_ref()
            .map(|content| review_comment_line_count(content_text(content)))
            .unwrap_or(0);
        if anchor_line > line_count {
            return Err(ServerError::InvalidRequest(format!(
                "review comment anchor_line {anchor_line} is outside the final review line range 1..={line_count}"
            )));
        }
    }
    user_ref(tx, author_user_id).await?;
    let comment_id = prefixed_id("cmt");
    sqlx::query(
        "INSERT INTO review_comments (
                comment_id, review_id, author_user_id, body, anchor_path, anchor_line,
                review_version
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&comment_id)
    .bind(review_id)
    .bind(author_user_id)
    .bind(&request.body)
    .bind(request.anchor_path.as_deref())
    .bind(request.anchor_line)
    .bind(review_version)
    .execute(&mut **tx)
    .await?;
    let comments = load_review_comments(tx, review_id).await?;
    let comment = comments
        .into_iter()
        .find(|comment| comment.comment_id == comment_id)
        .ok_or_else(|| ServerError::not_found("review_comment", &comment_id))?;
    Ok(comment)
}

pub(crate) async fn create_review_decision(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
    decided_by_user_id: &str,
    request: CreateReviewDecisionRequest,
) -> Result<ReviewDetail, ServerError> {
    let row = sqlx::query(
        "SELECT r.status AS review_status, r.version AS review_version
             FROM reviews r
             WHERE r.review_id = $1
             FOR UPDATE OF r",
    )
    .bind(review_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("review", review_id))?;
    let status: String = row.try_get("review_status")?;
    let version: i64 = row.try_get("review_version")?;

    if status != "open" {
        return Err(ServerError::invalid_transition(
            "review", &status, "decision",
        ));
    }
    if version != request.expected_review_version {
        return Err(ServerError::version_conflict(
            "review",
            request.expected_review_version,
            version,
        ));
    }
    let draft_ids = load_review_draft_ids(tx, review_id).await?;
    for draft_id in &draft_ids {
        let draft_status: String =
            sqlx::query_scalar("SELECT status FROM drafts WHERE draft_id = $1 FOR UPDATE")
                .bind(draft_id)
                .fetch_one(&mut **tx)
                .await?;
        if draft_status != "submitted" {
            return Err(ServerError::invalid_transition(
                "draft",
                &draft_status,
                "review_decided",
            ));
        }
    }

    let next_status = match request.decision {
        ReviewDecision::Approved => "approved",
        ReviewDecision::Rejected => "rejected",
    };
    let approved_result_hash = if request.decision == ReviewDecision::Approved {
        Some(review_result_hash(tx, &draft_ids).await?)
    } else {
        None
    };
    if request.decision == ReviewDecision::Rejected {
        for draft_id in &draft_ids {
            let reopened = sqlx::query(
                "UPDATE drafts
                     SET status = 'open', version = version + 1, updated_at = now()
                     WHERE draft_id = $1
                     RETURNING project_id, version",
            )
            .bind(draft_id)
            .fetch_one(&mut **tx)
            .await?;
            invalidate_draft_candidates(tx, draft_id).await?;
            insert_draft_event(
                tx,
                draft_id,
                &reopened.try_get::<String, _>("project_id")?,
                DraftEventType::Reopened,
                reopened.try_get("version")?,
                None,
            )
            .await?;
        }
    }
    sqlx::query(
        "UPDATE reviews
             SET status = $2, version = version + 1, decision_body = $3,
                 approved_result_hash = $4, decided_by_user_id = $5,
                 decided_at = now(), updated_at = now()
             WHERE review_id = $1",
    )
    .bind(review_id)
    .bind(next_status)
    .bind(&request.body)
    .bind(&approved_result_hash)
    .bind(decided_by_user_id)
    .execute(&mut **tx)
    .await?;

    let detail = load_review_detail(tx, review_id).await?;
    Ok(detail)
}

async fn missing_review_reconciliation_candidate(
    tx: &mut Transaction<'_, Postgres>,
    current_ref: &Option<String>,
    requests: &[ReviewDraftRequest],
) -> Result<Option<ServerError>, ServerError> {
    for request in requests {
        if request.candidate_id.is_some() {
            continue;
        }
        let row = sqlx::query("SELECT base_commit_id FROM drafts WHERE draft_id = $1 FOR UPDATE")
            .bind(&request.draft_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| ServerError::not_found("draft", &request.draft_id))?;
        if row.try_get::<Option<String>, _>("base_commit_id")? == *current_ref {
            continue;
        }
        let candidate = create_reconciliation_candidate_in_tx(
            tx,
            &request.draft_id,
            request.expected_draft_version,
        )
        .await?;
        return Ok(Some(ServerError::ReconciliationRequired {
            draft_id: request.draft_id.clone(),
            candidate_id: candidate.candidate_id,
            current_commit_id: candidate.current_commit_id,
        }));
    }
    Ok(None)
}

pub(crate) async fn create_review(
    tx: &mut Transaction<'_, Postgres>,
    author_user_id: &str,
    expected_ref: Option<&str>,
    request: CreateReviewRequest,
) -> Result<CommitOutcome<ReviewDetail>, ServerError> {
    let primary_request = request
        .drafts
        .first()
        .expect("service validates non-empty review drafts");
    let primary_draft_id = &primary_request.draft_id;
    let primary_expected_version = primary_request.expected_draft_version;
    let row = sqlx::query(
        "SELECT draft_id, project_id, author_user_id, title, description, status, version,
                    base_commit_id, resource_scope
             FROM drafts
             WHERE draft_id = $1
             FOR UPDATE",
    )
    .bind(primary_draft_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("draft", primary_draft_id))?;

    if row.try_get::<String, _>("author_user_id")? != author_user_id {
        return Err(ServerError::Forbidden(
            "only the draft author can create its review".to_owned(),
        ));
    }
    let status: String = row.try_get("status")?;
    let version: i64 = row.try_get("version")?;
    if status != "open" {
        return Err(ServerError::invalid_transition(
            "draft",
            &status,
            "submitted",
        ));
    }
    if version != primary_expected_version {
        return Err(ServerError::version_conflict(
            "draft",
            primary_expected_version,
            version,
        ));
    }
    let project_id: String = row.try_get("project_id")?;
    let scope = resource_scope(row.try_get::<String, _>("resource_scope")?.as_str())?;
    ensure_publishable_draft_scope(scope)?;
    let current_ref = target_ref_for_draft(tx, &project_id, scope).await?;
    if current_ref.as_deref() != expected_ref {
        return Err(ServerError::precondition_failed(
            expected_ref,
            current_ref.as_deref(),
        ));
    }
    if let Some(error) =
        missing_review_reconciliation_candidate(tx, &current_ref, &request.drafts).await?
    {
        return Ok(CommitOutcome::Failure(error));
    }
    let base_commit_id: Option<String> = row.try_get("base_commit_id")?;
    reconcile_review_draft(
        tx,
        author_user_id,
        expected_ref,
        &current_ref,
        primary_request,
        base_commit_id,
    )
    .await?;
    let operations = load_draft_operations(tx, primary_draft_id).await?;
    if operations.is_empty() {
        return Err(ServerError::InvalidRequest(
            "a review draft must contain at least one operation".to_owned(),
        ));
    }
    if scope == ResourceScope::Org {
        let org_id = project_org_id(tx, &project_id).await?;
        validate_stored_org_draft_operations_are_selected(
            tx,
            &project_id,
            &org_id,
            current_ref.as_deref(),
            &operations,
        )
        .await?;
    }

    for requested in request.drafts.iter().skip(1) {
        let additional = sqlx::query(
            "SELECT project_id, author_user_id, status, version, base_commit_id, resource_scope
                 FROM drafts WHERE draft_id = $1 FOR UPDATE",
        )
        .bind(&requested.draft_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ServerError::not_found("draft", &requested.draft_id))?;
        if additional.try_get::<String, _>("author_user_id")? != author_user_id {
            return Err(ServerError::Forbidden(
                "only the draft author can create its review".to_owned(),
            ));
        }
        let additional_status: String = additional.try_get("status")?;
        if additional_status != "open" {
            return Err(ServerError::invalid_transition(
                "draft",
                &additional_status,
                "submitted",
            ));
        }
        let actual_version: i64 = additional.try_get("version")?;
        if actual_version != requested.expected_draft_version {
            return Err(ServerError::version_conflict(
                "draft",
                requested.expected_draft_version,
                actual_version,
            ));
        }
        if additional.try_get::<String, _>("project_id")? != project_id {
            return Err(ServerError::InvalidRequest(
                "all drafts in a review must belong to the same project".to_owned(),
            ));
        }
        let additional_scope =
            resource_scope(additional.try_get::<String, _>("resource_scope")?.as_str())?;
        ensure_publishable_draft_scope(additional_scope)?;
        if additional_scope != scope {
            return Err(ServerError::InvalidRequest(
                "all drafts in a review must use the same scope".to_owned(),
            ));
        }
        let base_commit_id: Option<String> = additional.try_get("base_commit_id")?;
        reconcile_review_draft(
            tx,
            author_user_id,
            expected_ref,
            &current_ref,
            requested,
            base_commit_id,
        )
        .await?;
        let additional_operations = load_draft_operations(tx, &requested.draft_id).await?;
        if additional_operations.is_empty() {
            return Err(ServerError::InvalidRequest(
                "a review draft must contain at least one operation".to_owned(),
            ));
        }
        if scope == ResourceScope::Org {
            let org_id = project_org_id(tx, &project_id).await?;
            validate_stored_org_draft_operations_are_selected(
                tx,
                &project_id,
                &org_id,
                current_ref.as_deref(),
                &additional_operations,
            )
            .await?;
        }
    }

    let review_id = prefixed_id("rev");
    let fallback_title: String = row.try_get("title")?;
    let fallback_description: String = row.try_get("description")?;
    let title = request.title.unwrap_or(fallback_title);
    let description = request.description.unwrap_or(fallback_description);

    let draft_event_row = sqlx::query(
        "UPDATE drafts
             SET status = 'submitted', version = version + 1, updated_at = now()
             WHERE draft_id = $1
             RETURNING project_id, version",
    )
    .bind(primary_draft_id)
    .fetch_one(&mut **tx)
    .await?;
    invalidate_draft_candidates(tx, primary_draft_id).await?;
    insert_draft_event(
        tx,
        primary_draft_id,
        &draft_event_row.try_get::<String, _>("project_id")?,
        DraftEventType::Submitted,
        draft_event_row.try_get("version")?,
        None,
    )
    .await?;

    for requested in request.drafts.iter().skip(1) {
        let additional_event_row = sqlx::query(
            "UPDATE drafts
                 SET status = 'submitted', version = version + 1, updated_at = now()
                 WHERE draft_id = $1
                 RETURNING project_id, version",
        )
        .bind(&requested.draft_id)
        .fetch_one(&mut **tx)
        .await?;
        invalidate_draft_candidates(tx, &requested.draft_id).await?;
        insert_draft_event(
            tx,
            &requested.draft_id,
            &additional_event_row.try_get::<String, _>("project_id")?,
            DraftEventType::Submitted,
            additional_event_row.try_get("version")?,
            None,
        )
        .await?;
    }

    sqlx::query(
        "INSERT INTO reviews (
                review_id, draft_id, project_id, author_user_id, title, description,
                status, version
             )
             VALUES ($1, $2, $3, $4, $5, $6, 'open', 1)",
    )
    .bind(&review_id)
    .bind(primary_draft_id)
    .bind(&project_id)
    .bind(author_user_id)
    .bind(&title)
    .bind(&description)
    .execute(&mut **tx)
    .await?;

    for (ordinal, requested) in request.drafts.iter().enumerate() {
        sqlx::query(
            "INSERT INTO review_drafts (review_id, draft_id, ordinal)
                 VALUES ($1, $2, $3)",
        )
        .bind(&review_id)
        .bind(&requested.draft_id)
        .bind(ordinal as i32)
        .execute(&mut **tx)
        .await?;
    }

    let detail = load_review_detail(tx, &review_id).await?;
    Ok(CommitOutcome::Success(detail))
}

pub(crate) async fn create_review_submission(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
    author_user_id: &str,
    expected_ref: Option<&str>,
    request: CreateReviewSubmissionRequest,
) -> Result<CommitOutcome<ReviewDetail>, ServerError> {
    let Some(primary_request) = request.drafts.first() else {
        return Err(ServerError::InvalidRequest(
            "a review must contain at least one draft".to_owned(),
        ));
    };
    let primary_expected_version = primary_request.expected_draft_version;
    let distinct_draft_ids = request
        .drafts
        .iter()
        .map(|draft| &draft.draft_id)
        .collect::<BTreeSet<_>>();
    if distinct_draft_ids.len() != request.drafts.len() {
        return Err(ServerError::InvalidRequest(
            "a review must not contain duplicate drafts".to_owned(),
        ));
    }
    let row = sqlx::query(
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
    .await?
    .ok_or_else(|| ServerError::not_found("review", review_id))?;

    let draft_author_user_id: String = row.try_get("author_user_id")?;
    if draft_author_user_id != author_user_id {
        return Err(ServerError::Forbidden(
            "only the draft author can resubmit its review".to_owned(),
        ));
    }
    let review_status: String = row.try_get("review_status")?;
    if review_status != "rejected" {
        return Err(ServerError::invalid_transition(
            "review",
            &review_status,
            "resubmitted",
        ));
    }
    let review_version: i64 = row.try_get("review_version")?;
    if review_version != request.expected_review_version {
        return Err(ServerError::version_conflict(
            "review",
            request.expected_review_version,
            review_version,
        ));
    }
    let draft_status: String = row.try_get("draft_status")?;
    if draft_status != "open" {
        return Err(ServerError::invalid_transition(
            "draft",
            &draft_status,
            "submitted",
        ));
    }
    let draft_version: i64 = row.try_get("draft_version")?;
    if draft_version != primary_expected_version {
        return Err(ServerError::version_conflict(
            "draft",
            primary_expected_version,
            draft_version,
        ));
    }

    let draft_id: String = row.try_get("draft_id")?;
    if primary_request.draft_id != draft_id {
        return Err(ServerError::InvalidRequest(
            "a resubmission must keep the review's primary draft first".to_owned(),
        ));
    }
    let project_id: String = row.try_get("project_id")?;
    let scope = resource_scope(row.try_get::<String, _>("resource_scope")?.as_str())?;
    ensure_publishable_draft_scope(scope)?;
    let current_ref = target_ref_for_draft(tx, &project_id, scope).await?;
    if current_ref.as_deref() != expected_ref {
        return Err(ServerError::precondition_failed(
            expected_ref,
            current_ref.as_deref(),
        ));
    }
    if let Some(error) =
        missing_review_reconciliation_candidate(tx, &current_ref, &request.drafts).await?
    {
        return Ok(CommitOutcome::Failure(error));
    }
    let base_commit_id: Option<String> = row.try_get("base_commit_id")?;
    reconcile_review_draft(
        tx,
        author_user_id,
        expected_ref,
        &current_ref,
        primary_request,
        base_commit_id,
    )
    .await?;
    let operations = load_draft_operations(tx, &draft_id).await?;
    if operations.is_empty() {
        return Err(ServerError::InvalidRequest(
            "a review draft must contain at least one operation".to_owned(),
        ));
    }
    if scope == ResourceScope::Org {
        let org_id = project_org_id(tx, &project_id).await?;
        validate_stored_org_draft_operations_are_selected(
            tx,
            &project_id,
            &org_id,
            current_ref.as_deref(),
            &operations,
        )
        .await?;
    }
    for requested in request.drafts.iter().skip(1) {
        let linked_review_id: Option<String> =
            sqlx::query_scalar("SELECT review_id FROM review_drafts WHERE draft_id = $1")
                .bind(&requested.draft_id)
                .fetch_optional(&mut **tx)
                .await?;
        if linked_review_id
            .as_deref()
            .is_some_and(|linked_review_id| linked_review_id != review_id)
        {
            return Err(ServerError::already_exists(
                "review for draft",
                &requested.draft_id,
            ));
        }
        let additional = sqlx::query(
            "SELECT project_id, author_user_id, status, version, base_commit_id, resource_scope
                 FROM drafts WHERE draft_id = $1 FOR UPDATE",
        )
        .bind(&requested.draft_id)
        .fetch_one(&mut **tx)
        .await?;
        if additional.try_get::<String, _>("author_user_id")? != author_user_id {
            return Err(ServerError::Forbidden(
                "only the draft author can resubmit its review".to_owned(),
            ));
        }
        let additional_status: String = additional.try_get("status")?;
        if additional_status != "open" {
            return Err(ServerError::invalid_transition(
                "draft",
                &additional_status,
                "submitted",
            ));
        }
        let actual_version: i64 = additional.try_get("version")?;
        if actual_version != requested.expected_draft_version {
            return Err(ServerError::version_conflict(
                "draft",
                requested.expected_draft_version,
                actual_version,
            ));
        }
        if additional.try_get::<String, _>("project_id")? != project_id
            || resource_scope(additional.try_get::<String, _>("resource_scope")?.as_str())? != scope
        {
            return Err(ServerError::InvalidRequest(
                "all drafts in a review must share one project and scope".to_owned(),
            ));
        }
        let base_commit_id: Option<String> = additional.try_get("base_commit_id")?;
        reconcile_review_draft(
            tx,
            author_user_id,
            expected_ref,
            &current_ref,
            requested,
            base_commit_id,
        )
        .await?;
        let operations = load_draft_operations(tx, &requested.draft_id).await?;
        if operations.is_empty() {
            return Err(ServerError::InvalidRequest(
                "a review draft must contain at least one operation".to_owned(),
            ));
        }
        if scope == ResourceScope::Org {
            let org_id = project_org_id(tx, &project_id).await?;
            validate_stored_org_draft_operations_are_selected(
                tx,
                &project_id,
                &org_id,
                current_ref.as_deref(),
                &operations,
            )
            .await?;
        }
    }
    let next_draft_version: i64 = sqlx::query_scalar(
        "UPDATE drafts
             SET status = 'submitted', version = version + 1, updated_at = now()
             WHERE draft_id = $1
             RETURNING version",
    )
    .bind(&draft_id)
    .fetch_one(&mut **tx)
    .await?;
    invalidate_draft_candidates(tx, &draft_id).await?;
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
    .bind(request.title)
    .bind(request.description)
    .execute(&mut **tx)
    .await?;
    insert_draft_event(
        tx,
        &draft_id,
        &project_id,
        DraftEventType::Submitted,
        next_draft_version,
        None,
    )
    .await?;
    for requested in request.drafts.iter().skip(1) {
        let submitted = sqlx::query(
            "UPDATE drafts
                 SET status = 'submitted', version = version + 1, updated_at = now()
                 WHERE draft_id = $1
                 RETURNING project_id, version",
        )
        .bind(&requested.draft_id)
        .fetch_one(&mut **tx)
        .await?;
        invalidate_draft_candidates(tx, &requested.draft_id).await?;
        insert_draft_event(
            tx,
            &requested.draft_id,
            &submitted.try_get::<String, _>("project_id")?,
            DraftEventType::Submitted,
            submitted.try_get("version")?,
            None,
        )
        .await?;
    }

    sqlx::query("DELETE FROM review_drafts WHERE review_id = $1")
        .bind(review_id)
        .execute(&mut **tx)
        .await?;
    for (ordinal, requested) in request.drafts.iter().enumerate() {
        sqlx::query(
            "INSERT INTO review_drafts (review_id, draft_id, ordinal)
                 VALUES ($1, $2, $3)",
        )
        .bind(review_id)
        .bind(&requested.draft_id)
        .bind(ordinal as i32)
        .execute(&mut **tx)
        .await?;
    }

    let detail = load_review_detail(tx, review_id).await?;
    Ok(CommitOutcome::Success(detail))
}

pub(crate) async fn create_review_merge(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
    actor_user_id: &str,
    expected_project_ref: Option<&str>,
    request: CreateReviewMergeRequest,
) -> Result<CommitOutcome<ReviewMergeData>, ServerError> {
    let coordination = sqlx::query(
        "SELECT draft.project_id, draft.resource_scope
             FROM reviews AS review
             JOIN drafts AS draft ON draft.draft_id = review.draft_id
             WHERE review.review_id = $1",
    )
    .bind(review_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(coordination) = coordination
        && resource_scope(
            coordination
                .try_get::<String, _>("resource_scope")?
                .as_str(),
        )? == ResourceScope::Org
    {
        lock_org_draft_selection_coordination_for_project(
            tx,
            &coordination.try_get::<String, _>("project_id")?,
        )
        .await?;
    }
    let row = sqlx::query(
        "SELECT r.project_id, r.status, r.version, r.approved_result_hash
             FROM reviews r
             WHERE r.review_id = $1
             FOR UPDATE OF r",
    )
    .bind(review_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ServerError::not_found("review", review_id))?;

    let status: String = row.try_get("status")?;
    let version: i64 = row.try_get("version")?;
    if status != "open" && status != "approved" {
        return Err(ServerError::invalid_transition("review", &status, "merged"));
    }
    if version != request.expected_review_version {
        return Err(ServerError::version_conflict(
            "review",
            request.expected_review_version,
            version,
        ));
    }
    let project_id: String = row.try_get("project_id")?;
    let draft_ids = load_review_draft_ids(tx, review_id).await?;
    let mut draft_rows = Vec::with_capacity(draft_ids.len());
    for draft_id in &draft_ids {
        draft_rows.push(
            sqlx::query(
                "SELECT resource_scope, base_commit_id, status, version
                     FROM drafts WHERE draft_id = $1 FOR UPDATE",
            )
            .bind(draft_id)
            .fetch_one(&mut **tx)
            .await?,
        );
    }
    let primary_scope = resource_scope(
        draft_rows
            .first()
            .ok_or_else(|| ServerError::InvalidRequest("a review must contain a draft".to_owned()))?
            .try_get::<String, _>("resource_scope")?
            .as_str(),
    )?;
    ensure_publishable_draft_scope(primary_scope)?;
    for draft_row in &draft_rows {
        let scope = resource_scope(draft_row.try_get::<String, _>("resource_scope")?.as_str())?;
        if scope != primary_scope {
            return Err(ServerError::InvalidRequest(
                "all drafts in a review must use the same scope".to_owned(),
            ));
        }
        let draft_status: String = draft_row.try_get("status")?;
        if draft_status != "submitted" {
            return Err(ServerError::invalid_transition(
                "draft",
                &draft_status,
                "merged",
            ));
        }
    }
    let org_id = project_org_id(tx, &project_id).await?;
    let current_head = match primary_scope {
        ResourceScope::Org => current_org_ref(tx, &org_id).await?,
        ResourceScope::Project => {
            lock_org_ref_for_project_projection(tx, &org_id).await?;
            current_project_ref(tx, &project_id).await?
        }
    };
    if current_head.as_deref() != expected_project_ref {
        return Err(ServerError::precondition_failed(
            expected_project_ref,
            current_head.as_deref(),
        ));
    }
    for (draft_id, draft_row) in draft_ids.iter().zip(&draft_rows) {
        let draft_base_commit_id: Option<String> = draft_row.try_get("base_commit_id")?;
        if draft_base_commit_id != current_head {
            let candidate =
                create_reconciliation_candidate_in_tx(tx, draft_id, draft_row.try_get("version")?)
                    .await?;
            let error = ServerError::ReconciliationRequired {
                draft_id: draft_id.clone(),
                candidate_id: candidate.candidate_id,
                current_commit_id: candidate.current_commit_id,
            };
            return Ok(CommitOutcome::Failure(error));
        }
    }

    let approved_result_hash: Option<String> = row.try_get("approved_result_hash")?;
    let current_result_hash = review_result_hash(tx, &draft_ids).await?;
    if status == "approved" && approved_result_hash.as_deref() != Some(&current_result_hash) {
        return Err(ServerError::InvalidTransition {
            entity: "review",
            from: "approval_for_previous_content".to_owned(),
            to: "merged".to_owned(),
        });
    }

    let mut materialized_operations = Vec::new();
    for draft_id in &draft_ids {
        let operations = load_draft_operations(tx, draft_id).await?;
        materialized_operations.extend(materialize_draft_operations(&operations)?);
    }
    if primary_scope == ResourceScope::Org {
        validate_org_draft_operation_inputs_are_selected(
            tx,
            &project_id,
            &org_id,
            current_head.as_deref(),
            &materialized_operations,
        )
        .await?;
    }
    let org_resource_impact = match primary_scope {
        ResourceScope::Org => {
            resolve_org_resource_impact(tx, &org_id, &materialized_operations).await?
        }
        ResourceScope::Project => OrgResourceImpact::default(),
    };
    let mut created_resource_ids = Vec::new();
    for operation in &materialized_operations {
        if let Some(resource_id) =
            apply_operation(tx, &project_id, primary_scope, operation).await?
        {
            created_resource_ids.push(resource_id);
        }
    }

    let commit_id = match primary_scope {
        ResourceScope::Org => {
            let commit_id = create_org_commit(tx, &org_id, current_head.as_deref()).await?;
            advance_org_ref(tx, &org_id, &commit_id).await?;
            refresh_projects_for_org_resource_changes(tx, &org_id, &org_resource_impact).await?;
            if !created_resource_ids.is_empty() {
                select_created_org_resources_for_project(
                    tx,
                    &project_id,
                    &org_id,
                    &created_resource_ids,
                )
                .await?;
            }
            commit_id
        }
        ResourceScope::Project => {
            let commit_id = create_project_commit(tx, &project_id, current_head.as_deref()).await?;
            advance_project_ref(tx, &project_id, &commit_id).await?;
            commit_id
        }
    };
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
    .bind(status == "open")
    .bind(&current_result_hash)
    .bind(actor_user_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO review_merges (
                merge_id, review_id, commit_id, applied_operation_count
             )
             VALUES ($1, $2, $3, $4)",
    )
    .bind(prefixed_id("mrg"))
    .bind(review_id)
    .bind(&commit_id)
    .bind(materialized_operations.len() as i32)
    .execute(&mut **tx)
    .await?;
    for draft_id in &draft_ids {
        let merged_draft_version: i64 = sqlx::query_scalar(
            "UPDATE drafts
                 SET status = 'merged', version = version + 1, updated_at = now()
                 WHERE draft_id = $1
                 RETURNING version",
        )
        .bind(draft_id)
        .fetch_one(&mut **tx)
        .await?;
        insert_draft_event(
            tx,
            draft_id,
            &project_id,
            DraftEventType::Merged,
            merged_draft_version,
            None,
        )
        .await?;
    }

    Ok(CommitOutcome::Success(ReviewMergeData {
        commit_id,
        applied_operation_count: materialized_operations.len() as i64,
    }))
}
