//! Review use cases, authorization, and caller-owned publication transactions.

use super::repository::load_review_comments;
use super::{
    model::{ReviewMergeData, review_comment_line_count},
    repository,
};
use crate::app::auth::AuthPrincipal;
use crate::app::commit::{
    advance_org_ref, advance_project_ref, create_org_commit, create_project_commit,
    current_org_ref, current_project_ref, lock_org_ref_for_project_projection,
};
use crate::app::draft;
use crate::app::draft::dto::{CreateDraftRebaseRequest, DraftEventType};
use crate::app::draft::model::{
    CommitOutcome, aggregate_draft_coordination, content_text, ensure_publishable_draft_scope,
    materialize_draft_operations,
};
use crate::app::draft::{
    apply_draft_rebase_in_tx, apply_operation, create_reconciliation_candidate_in_tx,
    draft_result_hash, draft_result_state, insert_draft_event, invalidate_draft_candidates,
    load_draft_detail, load_draft_operations, target_ref_for_draft, user_ref,
    validate_org_draft_operation_inputs_are_selected,
    validate_stored_org_draft_operations_are_selected,
};
use crate::app::memory::dto::ResourceScope;
use crate::app::memory::model::{OrgResourceImpact, resource_scope};
use crate::app::memory::{
    lock_org_draft_selection_coordination_for_project, project_org_id,
    refresh_projects_for_org_resource_changes, resolve_org_resource_impact,
    select_created_org_resources_for_project,
};
use crate::app::review::dto::{
    CreateReviewCommentRequest, CreateReviewDecisionRequest, CreateReviewMergeRequest,
    CreateReviewRequest, CreateReviewSubmissionRequest, Review, ReviewComment,
    ReviewCommentListResponse, ReviewDecision, ReviewDetail, ReviewDraftDetail, ReviewDraftRequest,
    ReviewListResponse, ReviewMergeResult,
};
use crate::error::ServerError;
use crate::identity::prefixed_id;
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Transaction};
use std::collections::BTreeSet;

/// Hide reviews outside the principal's organization or accessible projects.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
async fn ensure_review_member(
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

/// Validate an author's proposal set and create or resubmit its review in one transaction.
///
/// # Errors
/// Rejects inaccessible or foreign-authored proposals, invalid proposal sets, stale revisions,
/// and invalid lifecycle changes. Reconciliation-required failures preserve the generated
/// candidate before returning; other failures leave the transaction uncommitted.
pub async fn create_review(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    expected_ref: Option<&str>,
    request: CreateReviewRequest,
) -> Result<ReviewDetail, ServerError> {
    let author_user_id = &principal.user_id;

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
            principal,
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
    draft::ensure_drafts_authored_by(&mut tx, author_user_id, &draft_ids).await?;
    let outcome = create_review_in_tx(&mut tx, author_user_id, expected_ref, request).await?;
    tx.commit().await?;
    outcome.into_result()
}

/// Return reviews visible through the principal's project memberships.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
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

/// Return review metadata only after checking the principal's project access.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub async fn get_review(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    review_id: &str,
) -> Result<Review, ServerError> {
    ensure_review_member(pool, principal, review_id).await?;

    let mut tx = pool.begin().await?;
    let review = load_review(&mut tx, review_id).await?;
    tx.commit().await?;
    Ok(review)
}

/// Return accessible review metadata, ordered proposal details, and discussion.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub async fn get_review_detail(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    review_id: &str,
) -> Result<ReviewDetail, ServerError> {
    ensure_review_member(pool, principal, review_id).await?;

    let mut tx = pool.begin().await?;
    let detail = load_review_detail(&mut tx, review_id).await?;
    tx.commit().await?;
    Ok(detail)
}

/// Return discussion only for a review accessible to the principal.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub async fn list_review_comments(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
    review_id: &str,
) -> Result<ReviewCommentListResponse, ServerError> {
    ensure_review_member(pool, principal, review_id).await?;

    let mut tx = pool.begin().await?;
    repository::ensure_review_exists(&mut tx, review_id).await?;
    let comments = load_review_comments(&mut tx, review_id).await?;
    tx.commit().await?;
    Ok(ReviewCommentListResponse {
        items: comments,
        page_info: crate::pagination::page_info(),
    })
}

/// Validate access, expected revision, and final-content anchors before persisting discussion.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, invalid input or lifecycle
/// state, and propagates persistence failures.
pub async fn create_review_comment(
    pool: &sqlx::PgPool,
    review_id: &str,
    principal: &AuthPrincipal,
    request: CreateReviewCommentRequest,
) -> Result<ReviewComment, ServerError> {
    let author_user_id = &principal.user_id;
    ensure_review_member(pool, principal, review_id).await?;

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
    let comment = create_review_comment_in_tx(&mut tx, review_id, author_user_id, request).await?;
    tx.commit().await?;
    Ok(comment)
}

/// Require organization administration and project access before deciding the submitted content.
///
/// # Errors
/// Rejects an identity outside the resource authorization boundary, and propagates persistence
/// failures.
pub async fn create_review_decision(
    pool: &sqlx::PgPool,
    review_id: &str,
    principal: &AuthPrincipal,
    request: CreateReviewDecisionRequest,
) -> Result<ReviewDetail, ServerError> {
    let decided_by_user_id = &principal.user_id;
    principal.require_org_admin()?;
    ensure_review_member(pool, principal, review_id).await?;

    let mut tx = pool.begin().await?;
    let detail =
        create_review_decision_in_tx(&mut tx, review_id, decided_by_user_id, request).await?;
    tx.commit().await?;
    Ok(detail)
}

/// Validate access and proposal ownership before resubmitting a rejected review.
///
/// # Errors
/// Rejects inaccessible or foreign-authored proposals, invalid resubmission state, and stale
/// revisions. Required reconciliation evidence is committed before its conflict is returned;
/// other failures do not commit the submission.
pub async fn create_review_submission(
    pool: &sqlx::PgPool,
    review_id: &str,
    principal: &AuthPrincipal,
    expected_ref: Option<&str>,
    request: CreateReviewSubmissionRequest,
) -> Result<ReviewDetail, ServerError> {
    let author_user_id = &principal.user_id;
    ensure_review_member(pool, principal, review_id).await?;

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
    draft::ensure_drafts_authored_by(&mut tx, author_user_id, &draft_ids).await?;
    let outcome =
        create_review_submission_in_tx(&mut tx, review_id, author_user_id, expected_ref, request)
            .await?;
    tx.commit().await?;
    outcome.into_result()
}

/// Authorize publication and commit resource changes, reference advancement, and synchronization
/// events atomically.
///
/// # Errors
/// Rejects unauthorized publication, changed review or reference revisions, invalid proposal
/// state, and persistence failures. A required reconciliation candidate is committed before
/// reporting its conflict; incomplete publication is not committed.
pub async fn create_review_merge(
    pool: &sqlx::PgPool,
    review_id: &str,
    principal: &AuthPrincipal,
    expected_project_ref: Option<&str>,
    request: CreateReviewMergeRequest,
) -> Result<ReviewMergeResult, ServerError> {
    let actor_user_id = &principal.user_id;
    principal.require_org_admin()?;
    ensure_review_member(pool, principal, review_id).await?;

    let mut tx = pool.begin().await?;
    let outcome = create_review_merge_in_tx(
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
    let review = get_review(pool, principal, review_id).await?;
    Ok(ReviewMergeResult {
        review,
        commit_id: Some(merge.commit_id),
        applied_operation_count: merge.applied_operation_count,
    })
}

/// Fingerprint all reviewed proposal results in their stable order for content-bound approval.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
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

/// Assemble review metadata using the current coordination state of its proposals.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub(crate) async fn load_review(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
) -> Result<Review, ServerError> {
    Ok(load_review_with_drafts(tx, review_id).await?.0)
}

/// Assemble all proposal details and discussion belonging to a review.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects invalid input or lifecycle state, and propagates persistence failures.
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

/// Load reviewed proposal details in the supplied stable review order.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
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

/// Require a valid candidate for upstream changes and apply the submitted conflict resolution.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects stale revisions or reference preconditions, invalid input or lifecycle state, and
/// propagates persistence failures.
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

use super::repository::load_review_draft_ids;

/// Preserve approval only while the changed proposal still matches the approved result
/// fingerprint.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub(crate) async fn refresh_review_after_draft_content_change(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: &str,
) -> Result<(), ServerError> {
    let approved_hash = repository::lock_approved_hash(tx, draft_id)
        .await?
        .flatten();
    let new_hash = if approved_hash.is_some() {
        Some(draft_result_hash(tx, draft_id).await?)
    } else {
        None
    };
    let preserve_approval = approved_hash.is_some() && approved_hash == new_hash;
    repository::update_approval_after_content_change(tx, draft_id, preserve_approval).await?;
    Ok(())
}

/// Join review metadata with proposal details and their aggregate freshness state.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates missing required state, invalid stored values, and persistence failures from the
/// participating resource operations.
pub(crate) async fn load_review_with_drafts(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
) -> Result<(Review, Vec<ReviewDraftDetail>), ServerError> {
    let draft_ids = repository::load_review_draft_ids(tx, review_id).await?;
    let drafts = load_review_drafts(tx, &draft_ids).await?;
    let coordinations = drafts
        .iter()
        .map(|detail| detail.draft.coordination.clone())
        .collect::<Vec<_>>();
    let review = repository::load_review(
        tx,
        review_id,
        draft_ids,
        aggregate_draft_coordination(&coordinations),
    )
    .await?;
    Ok((review, drafts))
}

/// Validate a comment against the locked review revision and final reviewed content before
/// inserting it.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects stale revisions or reference preconditions, invalid input or lifecycle state, and
/// propagates persistence failures.
pub(crate) async fn create_review_comment_in_tx(
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
    let review_row = repository::lock_comment_version(tx, review_id)
        .await?
        .ok_or_else(|| ServerError::not_found("review", review_id))?;
    let review_version: i64 = review_row.version;
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
    repository::insert_comment(
        tx,
        repository::NewReviewComment {
            comment_id: &comment_id,
            review_id,
            author_user_id,
            body: &request.body,
            anchor_path: request.anchor_path.as_deref(),
            anchor_line: request.anchor_line,
            review_version,
        },
    )
    .await?;
    let comments = load_review_comments(tx, review_id).await?;
    let comment = comments
        .into_iter()
        .find(|comment| comment.comment_id == comment_id)
        .ok_or_else(|| ServerError::not_found("review_comment", &comment_id))?;
    Ok(comment)
}

/// Lock submitted proposals and atomically record approval or reopen proposals after rejection.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects stale revisions or reference preconditions, invalid input or lifecycle state, and
/// propagates persistence failures.
pub(crate) async fn create_review_decision_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
    decided_by_user_id: &str,
    request: CreateReviewDecisionRequest,
) -> Result<ReviewDetail, ServerError> {
    let row = repository::lock_decision_state(tx, review_id)
        .await?
        .ok_or_else(|| ServerError::not_found("review", review_id))?;
    let status: String = row.review_status.clone();
    let version: i64 = row.review_version;

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
        let draft_status: String = repository::lock_draft_status(tx, draft_id).await?;
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
            let reopened = repository::reopen_draft(tx, draft_id).await?;
            invalidate_draft_candidates(tx, draft_id).await?;
            insert_draft_event(
                tx,
                draft_id,
                &reopened.project_id.clone(),
                DraftEventType::Reopened,
                reopened.version,
                None,
            )
            .await?;
        }
    }
    repository::update_decision(
        tx,
        review_id,
        next_status,
        &request.body,
        &approved_result_hash,
        decided_by_user_id,
    )
    .await?;

    let detail = load_review_detail(tx, review_id).await?;
    Ok(detail)
}

/// Persist reconciliation evidence for the first behind proposal submitted without a candidate.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects stale revisions or reference preconditions, and propagates persistence failures.
async fn missing_review_reconciliation_candidate(
    tx: &mut Transaction<'_, Postgres>,
    current_ref: &Option<String>,
    requests: &[ReviewDraftRequest],
) -> Result<Option<ServerError>, ServerError> {
    for request in requests {
        if request.candidate_id.is_some() {
            continue;
        }
        let row = repository::lock_draft_base(tx, &request.draft_id)
            .await?
            .ok_or_else(|| ServerError::not_found("draft", &request.draft_id))?;
        if row.base_commit_id.clone() == *current_ref {
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

/// Validate proposal ownership, lifecycle, common scope, and reconciliation before recording
/// submission.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects invalid proposal ownership, state, scope, or revisions and propagates persistence
/// failures. A returned failure outcome carries evidence that the caller must commit before
/// exposing the conflict.
pub(crate) async fn create_review_in_tx(
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
    let row = repository::lock_primary_draft(tx, primary_draft_id)
        .await?
        .ok_or_else(|| ServerError::not_found("draft", primary_draft_id))?;

    if row.author_user_id.clone() != author_user_id {
        return Err(ServerError::Forbidden(
            "only the draft author can create its review".to_owned(),
        ));
    }
    let status: String = row.status.clone();
    let version: i64 = row.version;
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
    let project_id: String = row.project_id.clone();
    let scope = resource_scope(row.resource_scope.clone().as_str())?;
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
    let base_commit_id: Option<String> = row.base_commit_id.clone();
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
        let additional = repository::lock_additional_draft(tx, &requested.draft_id)
            .await?
            .ok_or_else(|| ServerError::not_found("draft", &requested.draft_id))?;
        if additional.author_user_id.clone() != author_user_id {
            return Err(ServerError::Forbidden(
                "only the draft author can create its review".to_owned(),
            ));
        }
        let additional_status: String = additional.status.clone();
        if additional_status != "open" {
            return Err(ServerError::invalid_transition(
                "draft",
                &additional_status,
                "submitted",
            ));
        }
        let actual_version: i64 = additional.version;
        if actual_version != requested.expected_draft_version {
            return Err(ServerError::version_conflict(
                "draft",
                requested.expected_draft_version,
                actual_version,
            ));
        }
        if additional.project_id.clone() != project_id {
            return Err(ServerError::InvalidRequest(
                "all drafts in a review must belong to the same project".to_owned(),
            ));
        }
        let additional_scope = resource_scope(additional.resource_scope.clone().as_str())?;
        ensure_publishable_draft_scope(additional_scope)?;
        if additional_scope != scope {
            return Err(ServerError::InvalidRequest(
                "all drafts in a review must use the same scope".to_owned(),
            ));
        }
        let base_commit_id: Option<String> = additional.base_commit_id.clone();
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
    let fallback_title: String = row.title.clone();
    let fallback_description: String = row.description.clone();
    let title = request.title.unwrap_or(fallback_title);
    let description = request.description.unwrap_or(fallback_description);

    let draft_event_row = repository::submit_draft(tx, primary_draft_id).await?;
    invalidate_draft_candidates(tx, primary_draft_id).await?;
    insert_draft_event(
        tx,
        primary_draft_id,
        &draft_event_row.project_id.clone(),
        DraftEventType::Submitted,
        draft_event_row.version,
        None,
    )
    .await?;

    for requested in request.drafts.iter().skip(1) {
        let additional_event_row = repository::submit_draft(tx, &requested.draft_id).await?;
        invalidate_draft_candidates(tx, &requested.draft_id).await?;
        insert_draft_event(
            tx,
            &requested.draft_id,
            &additional_event_row.project_id.clone(),
            DraftEventType::Submitted,
            additional_event_row.version,
            None,
        )
        .await?;
    }

    repository::insert_review(
        tx,
        &review_id,
        primary_draft_id,
        &project_id,
        author_user_id,
        &title,
        &description,
    )
    .await?;

    for (ordinal, requested) in request.drafts.iter().enumerate() {
        repository::insert_review_draft(tx, &review_id, &requested.draft_id, ordinal as i32)
            .await?;
    }

    let detail = load_review_detail(tx, &review_id).await?;
    Ok(CommitOutcome::Success(detail))
}

/// Revalidate rejected-review proposals and replace their ordered submission links atomically.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects invalid resubmission state, proposal ownership, or stale revisions. A reconciliation
/// failure outcome requires the caller to commit its generated evidence before returning the
/// conflict.
pub(crate) async fn create_review_submission_in_tx(
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
    let row = repository::lock_submission_state(tx, review_id)
        .await?
        .ok_or_else(|| ServerError::not_found("review", review_id))?;

    let draft_author_user_id: String = row.author_user_id.clone();
    if draft_author_user_id != author_user_id {
        return Err(ServerError::Forbidden(
            "only the draft author can resubmit its review".to_owned(),
        ));
    }
    let review_status: String = row.review_status.clone();
    if review_status != "rejected" {
        return Err(ServerError::invalid_transition(
            "review",
            &review_status,
            "resubmitted",
        ));
    }
    let review_version: i64 = row.review_version;
    if review_version != request.expected_review_version {
        return Err(ServerError::version_conflict(
            "review",
            request.expected_review_version,
            review_version,
        ));
    }
    let draft_status: String = row.draft_status.clone();
    if draft_status != "open" {
        return Err(ServerError::invalid_transition(
            "draft",
            &draft_status,
            "submitted",
        ));
    }
    let draft_version: i64 = row.draft_version;
    if draft_version != primary_expected_version {
        return Err(ServerError::version_conflict(
            "draft",
            primary_expected_version,
            draft_version,
        ));
    }

    let draft_id: String = row.draft_id.clone();
    if primary_request.draft_id != draft_id {
        return Err(ServerError::InvalidRequest(
            "a resubmission must keep the review's primary draft first".to_owned(),
        ));
    }
    let project_id: String = row.project_id.clone();
    let scope = resource_scope(row.resource_scope.clone().as_str())?;
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
    let base_commit_id: Option<String> = row.base_commit_id.clone();
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
            repository::find_draft_review(tx, &requested.draft_id).await?;
        if linked_review_id
            .as_deref()
            .is_some_and(|linked_review_id| linked_review_id != review_id)
        {
            return Err(ServerError::already_exists(
                "review for draft",
                &requested.draft_id,
            ));
        }
        let additional =
            repository::lock_required_additional_draft(tx, &requested.draft_id).await?;
        if additional.author_user_id.clone() != author_user_id {
            return Err(ServerError::Forbidden(
                "only the draft author can resubmit its review".to_owned(),
            ));
        }
        let additional_status: String = additional.status.clone();
        if additional_status != "open" {
            return Err(ServerError::invalid_transition(
                "draft",
                &additional_status,
                "submitted",
            ));
        }
        let actual_version: i64 = additional.version;
        if actual_version != requested.expected_draft_version {
            return Err(ServerError::version_conflict(
                "draft",
                requested.expected_draft_version,
                actual_version,
            ));
        }
        if additional.project_id.clone() != project_id
            || resource_scope(additional.resource_scope.clone().as_str())? != scope
        {
            return Err(ServerError::InvalidRequest(
                "all drafts in a review must share one project and scope".to_owned(),
            ));
        }
        let base_commit_id: Option<String> = additional.base_commit_id.clone();
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
    let next_draft_version: i64 = repository::resubmit_primary_draft(tx, &draft_id).await?;
    invalidate_draft_candidates(tx, &draft_id).await?;
    repository::reopen_review(tx, review_id, request.title, request.description).await?;
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
        let submitted = repository::submit_draft(tx, &requested.draft_id).await?;
        invalidate_draft_candidates(tx, &requested.draft_id).await?;
        insert_draft_event(
            tx,
            &requested.draft_id,
            &submitted.project_id.clone(),
            DraftEventType::Submitted,
            submitted.version,
            None,
        )
        .await?;
    }

    repository::delete_review_drafts(tx, review_id).await?;
    for (ordinal, requested) in request.drafts.iter().enumerate() {
        repository::insert_review_draft(tx, review_id, &requested.draft_id, ordinal as i32).await?;
    }

    let detail = load_review_detail(tx, review_id).await?;
    Ok(CommitOutcome::Success(detail))
}

/// Check locked review and proposal revisions before materializing content and advancing
/// authoritative references.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Rejects invalid review state, stale references or proposal revisions, approval changes, and
/// persistence failures. Reconciliation-required outcomes must commit their evidence without
/// publishing partial resource changes.
pub(crate) async fn create_review_merge_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
    actor_user_id: &str,
    expected_project_ref: Option<&str>,
    request: CreateReviewMergeRequest,
) -> Result<CommitOutcome<ReviewMergeData>, ServerError> {
    let coordination = repository::load_coordination(tx, review_id).await?;
    if let Some(coordination) = coordination
        && resource_scope(coordination.resource_scope.clone().as_str())? == ResourceScope::Org
    {
        lock_org_draft_selection_coordination_for_project(tx, &coordination.project_id.clone())
            .await?;
    }
    let row = repository::lock_merge_state(tx, review_id)
        .await?
        .ok_or_else(|| ServerError::not_found("review", review_id))?;

    let status: String = row.status.clone();
    let version: i64 = row.version;
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
    let project_id: String = row.project_id.clone();
    let draft_ids = load_review_draft_ids(tx, review_id).await?;
    let mut draft_rows = Vec::with_capacity(draft_ids.len());
    for draft_id in &draft_ids {
        draft_rows.push(repository::lock_merge_draft(tx, draft_id).await?);
    }
    let primary_scope = resource_scope(
        draft_rows
            .first()
            .ok_or_else(|| ServerError::InvalidRequest("a review must contain a draft".to_owned()))?
            .resource_scope
            .clone()
            .as_str(),
    )?;
    ensure_publishable_draft_scope(primary_scope)?;
    for draft_row in &draft_rows {
        let scope = resource_scope(draft_row.resource_scope.clone().as_str())?;
        if scope != primary_scope {
            return Err(ServerError::InvalidRequest(
                "all drafts in a review must use the same scope".to_owned(),
            ));
        }
        let draft_status: String = draft_row.status.clone();
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
        let draft_base_commit_id: Option<String> = draft_row.base_commit_id.clone();
        if draft_base_commit_id != current_head {
            let candidate =
                create_reconciliation_candidate_in_tx(tx, draft_id, draft_row.version).await?;
            let error = ServerError::ReconciliationRequired {
                draft_id: draft_id.clone(),
                candidate_id: candidate.candidate_id,
                current_commit_id: candidate.current_commit_id,
            };
            return Ok(CommitOutcome::Failure(error));
        }
    }

    let approved_result_hash: Option<String> = row.approved_result_hash.clone();
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
    repository::mark_merged(
        tx,
        review_id,
        status == "open",
        &current_result_hash,
        actor_user_id,
    )
    .await?;
    repository::insert_merge(
        tx,
        prefixed_id("mrg"),
        review_id,
        &commit_id,
        materialized_operations.len() as i32,
    )
    .await?;
    for draft_id in &draft_ids {
        let merged_draft_version: i64 = repository::merge_draft(tx, draft_id).await?;
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
