//! Application operations and transaction coordination for review resources.

use super::repository;
use super::repository::{load_review_comments, load_review_with_drafts};
use crate::app::auth::AuthPrincipal;
use crate::app::draft;
use crate::app::draft::dto::CreateDraftRebaseRequest;
use crate::app::draft::model::CommitOutcome;
use crate::app::draft::service::{apply_draft_rebase_in_tx, draft_result_hash, load_draft_detail};
use crate::app::review::dto::{
    CreateReviewCommentRequest, CreateReviewDecisionRequest, CreateReviewMergeRequest,
    CreateReviewRequest, CreateReviewSubmissionRequest, Review, ReviewComment,
    ReviewCommentListResponse, ReviewDetail, ReviewDraftDetail, ReviewDraftRequest,
    ReviewListResponse, ReviewMergeResult,
};
use crate::error::ServerError;
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Transaction};
use std::collections::BTreeSet;

pub async fn ensure_review_member(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    review_id: &str,
) -> Result<(), ServerError> {
    if repository::review_is_accessible(pool, principal, review_id).await? {
        Ok(())
    } else {
        Err(ServerError::not_found("review", review_id))
    }
}

pub async fn create_review(
    pool: &sqlx::PgPool,
    author_user_id: &str,
    expected_ref: Option<&str>,
    request: CreateReviewRequest,
) -> Result<ReviewDetail, ServerError> {
    let draft_ids = request
        .drafts
        .iter()
        .map(|draft| draft.draft_id.clone())
        .collect::<Vec<_>>();
    let Some(primary_draft_id) = draft_ids.first() else {
        return Err(ServerError::InvalidRequest(
            "a review must contain at least one draft".to_owned(),
        ));
    };
    let distinct_draft_ids = draft_ids.iter().collect::<BTreeSet<_>>();
    if distinct_draft_ids.len() != draft_ids.len() {
        return Err(ServerError::InvalidRequest(
            "a review must not contain duplicate drafts".to_owned(),
        ));
    }
    if let Some((review_id, version)) =
        repository::find_rejected_review(pool, primary_draft_id).await?
    {
        return create_review_submission(
            pool,
            &review_id,
            author_user_id,
            expected_ref,
            CreateReviewSubmissionRequest {
                expected_review_version: version,
                drafts: request.drafts,
                title: request.title,
                description: request.description,
            },
        )
        .await;
    }

    let mut tx = pool.begin().await?;
    draft::service::ensure_drafts_authored_by(&mut tx, author_user_id, &draft_ids).await?;
    let outcome = repository::create_review(&mut tx, author_user_id, expected_ref, request).await?;
    tx.commit().await?;
    outcome.into_result()
}

pub async fn list_reviews(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    project_id: Option<&str>,
) -> Result<ReviewListResponse, ServerError> {
    let mut tx = pool.begin().await?;
    let response = repository::list_reviews(&mut tx, principal, project_id).await?;
    tx.commit().await?;
    Ok(response)
}

pub async fn get_review(pool: &sqlx::PgPool, review_id: &str) -> Result<Review, ServerError> {
    let mut tx = pool.begin().await?;
    let review = load_review(&mut tx, review_id).await?;
    tx.commit().await?;
    Ok(review)
}

pub async fn get_review_detail(
    pool: &sqlx::PgPool,
    review_id: &str,
) -> Result<ReviewDetail, ServerError> {
    let mut tx = pool.begin().await?;
    let detail = load_review_detail(&mut tx, review_id).await?;
    tx.commit().await?;
    Ok(detail)
}

pub async fn list_review_comments(
    pool: &sqlx::PgPool,
    review_id: &str,
) -> Result<ReviewCommentListResponse, ServerError> {
    let mut tx = pool.begin().await?;
    repository::ensure_review_exists(&mut tx, review_id).await?;
    let comments = load_review_comments(&mut tx, review_id).await?;
    tx.commit().await?;
    Ok(ReviewCommentListResponse {
        items: comments,
        page_info: crate::pagination::page_info(),
    })
}

pub async fn create_review_comment(
    pool: &sqlx::PgPool,
    review_id: &str,
    author_user_id: &str,
    request: CreateReviewCommentRequest,
) -> Result<ReviewComment, ServerError> {
    if request.body.trim().is_empty() {
        return Err(ServerError::InvalidRequest(
            "review comment body must not be empty".to_owned(),
        ));
    }
    match (request.anchor_path.as_deref(), request.anchor_line) {
        (None, None) => {}
        (Some(path), Some(line)) if !path.is_empty() && line > 0 => {}
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
    }
    let mut tx = pool.begin().await?;
    let comment =
        repository::create_review_comment(&mut tx, review_id, author_user_id, request).await?;
    tx.commit().await?;
    Ok(comment)
}

pub async fn create_review_decision(
    pool: &sqlx::PgPool,
    review_id: &str,
    decided_by_user_id: &str,
    request: CreateReviewDecisionRequest,
) -> Result<ReviewDetail, ServerError> {
    let mut tx = pool.begin().await?;
    let detail =
        repository::create_review_decision(&mut tx, review_id, decided_by_user_id, request).await?;
    tx.commit().await?;
    Ok(detail)
}

pub async fn create_review_submission(
    pool: &sqlx::PgPool,
    review_id: &str,
    author_user_id: &str,
    expected_ref: Option<&str>,
    request: CreateReviewSubmissionRequest,
) -> Result<ReviewDetail, ServerError> {
    let Some(_) = request.drafts.first() else {
        return Err(ServerError::InvalidRequest(
            "a review must contain at least one draft".to_owned(),
        ));
    };
    let draft_ids = request
        .drafts
        .iter()
        .map(|draft| draft.draft_id.clone())
        .collect::<Vec<_>>();
    let distinct_draft_ids = draft_ids.iter().collect::<BTreeSet<_>>();
    if distinct_draft_ids.len() != draft_ids.len() {
        return Err(ServerError::InvalidRequest(
            "a review must not contain duplicate drafts".to_owned(),
        ));
    }
    let mut tx = pool.begin().await?;
    draft::service::ensure_drafts_authored_by(&mut tx, author_user_id, &draft_ids).await?;
    let outcome = repository::create_review_submission(
        &mut tx,
        review_id,
        author_user_id,
        expected_ref,
        request,
    )
    .await?;
    tx.commit().await?;
    outcome.into_result()
}

pub async fn create_review_merge(
    pool: &sqlx::PgPool,
    review_id: &str,
    actor_user_id: &str,
    expected_project_ref: Option<&str>,
    request: CreateReviewMergeRequest,
) -> Result<ReviewMergeResult, ServerError> {
    let mut tx = pool.begin().await?;
    let outcome = repository::create_review_merge(
        &mut tx,
        review_id,
        actor_user_id,
        expected_project_ref,
        request,
    )
    .await?;
    tx.commit().await?;
    let merge = match outcome {
        CommitOutcome::Success(merge) => merge,
        CommitOutcome::Failure(error) => return Err(error),
    };
    let review = get_review(pool, review_id).await?;
    Ok(ReviewMergeResult {
        review,
        commit_id: Some(merge.commit_id),
        applied_operation_count: merge.applied_operation_count,
    })
}

pub(crate) async fn review_result_hash(
    tx: &mut Transaction<'_, Postgres>,
    draft_ids: &[String],
) -> Result<String, ServerError> {
    if let [draft_id] = draft_ids {
        return draft_result_hash(tx, draft_id).await;
    }
    let mut hasher = Sha256::new();
    for draft_id in draft_ids {
        hasher.update(draft_id.as_bytes());
        hasher.update([0]);
        hasher.update(draft_result_hash(tx, draft_id).await?.as_bytes());
        hasher.update([0]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub(crate) async fn load_review(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
) -> Result<Review, ServerError> {
    Ok(load_review_with_drafts(tx, review_id).await?.0)
}

pub(crate) async fn load_review_detail(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
) -> Result<ReviewDetail, ServerError> {
    let (review, drafts) = load_review_with_drafts(tx, review_id).await?;
    let primary = drafts
        .first()
        .cloned()
        .ok_or_else(|| ServerError::InvalidRequest("a review must contain a draft".to_owned()))?;
    let comments = load_review_comments(tx, review_id).await?;
    Ok(ReviewDetail {
        review,
        draft: primary.draft,
        operations: primary.operations,
        drafts,
        comments,
    })
}

pub(crate) async fn load_review_drafts(
    tx: &mut Transaction<'_, Postgres>,
    draft_ids: &[String],
) -> Result<Vec<ReviewDraftDetail>, ServerError> {
    let mut drafts = Vec::with_capacity(draft_ids.len());
    for draft_id in draft_ids {
        let detail = load_draft_detail(tx, draft_id).await?;
        drafts.push(ReviewDraftDetail {
            draft: detail.draft,
            operations: detail.operations,
        });
    }
    Ok(drafts)
}

pub(crate) async fn reconcile_review_draft(
    tx: &mut Transaction<'_, Postgres>,
    author_user_id: &str,
    expected_ref: Option<&str>,
    current_ref: &Option<String>,
    request: &ReviewDraftRequest,
    base_commit_id: Option<String>,
) -> Result<(), ServerError> {
    if base_commit_id.as_deref() == current_ref.as_deref() {
        if request.candidate_id.is_some() || request.resolved_state.is_some() {
            return Err(ServerError::InvalidRequest(
                "a current draft must not submit reconciliation data".to_owned(),
            ));
        }
        return Ok(());
    }

    let candidate_id = request.candidate_id.clone().ok_or_else(|| {
        ServerError::InvalidRequest("a behind draft must submit reconciliation data".to_owned())
    })?;
    apply_draft_rebase_in_tx(
        tx,
        &request.draft_id,
        author_user_id,
        expected_ref,
        CreateDraftRebaseRequest {
            candidate_id,
            expected_draft_version: request.expected_draft_version,
            resolved_state: request.resolved_state.clone(),
        },
    )
    .await?;
    Ok(())
}

// Transaction participants share the caller-owned transaction; only the outer service commits.
pub(crate) use super::repository::refresh_review_after_draft_content_change;

pub(crate) use super::repository::load_review_draft_ids;
