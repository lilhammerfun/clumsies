//! Whole-review updates, transaction rollback, discarded membership, and legacy repair.

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde::{Serialize, de::DeserializeOwned};
use server::app::draft::dto::DraftDetail;
use server::app::review::dto::*;
use tower::ServiceExt;

mod common;

async fn post<T: DeserializeOwned>(
    app: &Router,
    path: &str,
    body: impl Serialize,
    head: Option<&str>,
) -> T {
    let (status, bytes) = request(app, "POST", path, body, head).await;
    assert!(
        status.is_success(),
        "{path}: {status}: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    body: impl Serialize,
    head: Option<&str>,
) -> (StatusCode, Vec<u8>) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .header("idempotency-key", uuid::Uuid::new_v4().to_string())
                .header("if-match", format!("\"{}\"", head.unwrap_or("ref-none")))
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (
        status,
        to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
}

async fn fixture() -> (common::TestPostgres, Router, String) {
    let pg = common::migrated_postgres().await;
    let installation = common::initialize_installation(
        pg.pool.clone(),
        "Review updates",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Review updates",
    )
    .await;
    let (app, _) = common::authenticated_router(pg.pool.clone()).await;
    (pg, app, installation.project_id)
}

async fn draft(app: &Router, project: &str, name: &str) -> DraftDetail {
    draft_content(app, project, name, None, &format!("# {name}\n")).await
}

async fn draft_content(
    app: &Router,
    project: &str,
    name: &str,
    base: Option<&str>,
    content: &str,
) -> DraftDetail {
    post(
        app,
        "/api/v1/drafts",
        serde_json::json!({
            "daemon_installation_id": "review-update-test", "project_id": project,
            "base_commit_id": base, "title": name,
            "resource": {"scope":"org","path":name},
            "operations":[{"action":if base.is_some() {"update"} else {"create"},
                           "resource":{"scope":"org","path":name},
                           "content":{"content":content} }]
        }),
        None,
    )
    .await
}

async fn review(app: &Router, drafts: &[DraftDetail]) -> ReviewDetail {
    post(
        app,
        "/api/v1/reviews",
        CreateReviewRequest {
            drafts: drafts
                .iter()
                .map(|d| ReviewDraftRequest {
                    draft_id: d.draft.draft_id.clone(),
                    expected_draft_version: d.draft.version,
                    candidate_id: None,
                    resolved_state: None,
                })
                .collect(),
            title: None,
            description: None,
        },
        drafts[0].draft.base_commit_id.as_deref(),
    )
    .await
}

async fn publish(app: &Router, review: &ReviewDetail, head: Option<&str>) -> ReviewMergeResult {
    post(
        app,
        &format!("/api/v1/reviews/{}/merges", review.review.review_id),
        CreateReviewMergeRequest {
            expected_review_version: review.review.version,
        },
        head,
    )
    .await
}

async fn discard(app: &Router, detail: &ReviewDetail, index: usize) {
    let draft = &detail.drafts[index].draft;
    let (status, body) = request(
        app,
        "DELETE",
        &format!("/api/v1/drafts/{}", draft.draft_id),
        serde_json::Value::Null,
        Some(&draft.version.to_string()),
    )
    .await;
    assert!(
        status.is_success(),
        "{status}: {}",
        String::from_utf8_lossy(&body)
    );
}

async fn get_review(app: &Router, id: &str) -> ReviewDetail {
    let (status, body) = request(app, "GET", &format!("/api/v1/reviews/{id}"), (), None).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn updates_all_remaining_files_atomically_then_allows_publication() {
    let (pg, app, project) = fixture().await;
    let first = draft(&app, &project, "first.md").await;
    let second = draft(&app, &project, "second.md").await;
    let removed = draft(&app, &project, "removed.md").await;
    let initial = review(&app, &[first, second, removed]).await;
    discard(&app, &initial, 2).await;
    let pending = get_review(&app, &initial.review.review_id).await;
    assert_eq!(pending.drafts.len(), 2);
    assert_eq!(pending.review.status, ReviewStatus::Open);
    assert!(pending.review.version > initial.review.version);

    let upstream = draft(&app, &project, "upstream.md").await;
    let upstream = review(&app, &[upstream]).await;
    let published: ReviewMergeResult = post(
        &app,
        &format!("/api/v1/reviews/{}/merges", upstream.review.review_id),
        CreateReviewMergeRequest {
            expected_review_version: upstream.review.version,
        },
        None,
    )
    .await;
    let head = published.commit_id.as_deref();
    let plan: ReviewUpdatePlan = post(
        &app,
        &format!("/api/v1/reviews/{}/update-plans", pending.review.review_id),
        CreateReviewUpdatePlanRequest {
            expected_review_version: pending.review.version,
        },
        head,
    )
    .await;
    assert_eq!(plan.candidates.len(), 2);
    let updates = CreateReviewUpdateRequest {
        expected_review_version: plan.detail.review.version,
        drafts: plan
            .detail
            .drafts
            .iter()
            .zip(&plan.candidates)
            .map(|(item, candidate)| ReviewDraftRequest {
                draft_id: item.draft.draft_id.clone(),
                expected_draft_version: item.draft.version,
                candidate_id: Some(candidate.candidate_id.clone()),
                resolved_state: None,
            })
            .collect(),
    };
    let mut stale = updates.clone();
    stale.drafts[1].expected_draft_version += 1;
    let (status, _) = request(
        &app,
        "POST",
        &format!("/api/v1/reviews/{}/updates", pending.review.review_id),
        stale,
        head,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let unchanged = get_review(&app, &pending.review.review_id).await;
    assert_eq!(unchanged.review.version, pending.review.version);
    assert_eq!(unchanged.drafts, plan.detail.drafts);
    let rebases: i64 = sqlx::query_scalar("SELECT count(*) FROM draft_rebases")
        .fetch_one(&pg.pool)
        .await
        .unwrap();
    assert_eq!(
        rebases, 0,
        "a failure on the second file must roll back the first file"
    );

    let updated: ReviewDetail = post(
        &app,
        &format!("/api/v1/reviews/{}/updates", pending.review.review_id),
        updates,
        head,
    )
    .await;
    assert_eq!(
        updated.review.coordination.freshness,
        server::app::draft::dto::DraftFreshness::Current
    );
    assert_eq!(
        updated.review.status,
        ReviewStatus::Open,
        "updating must not publish"
    );
    let merged: ReviewMergeResult = post(
        &app,
        &format!("/api/v1/reviews/{}/merges", pending.review.review_id),
        CreateReviewMergeRequest {
            expected_review_version: updated.review.version,
        },
        head,
    )
    .await;
    assert_eq!(merged.review.status, ReviewStatus::Merged);
    assert_eq!(merged.applied_operation_count, 2);
}

#[tokio::test]
async fn discarding_primary_keeps_remaining_files_and_invalidates_approval() {
    let (_pg, app, project) = fixture().await;
    let first = draft(&app, &project, "first.md").await;
    let second = draft(&app, &project, "second.md").await;
    let initial = review(&app, &[first, second]).await;
    let approved: ReviewDetail = post(
        &app,
        &format!("/api/v1/reviews/{}/decisions", initial.review.review_id),
        CreateReviewDecisionRequest {
            decision: ReviewDecision::Approved,
            expected_review_version: initial.review.version,
            body: None,
        },
        None,
    )
    .await;
    discard(&app, &approved, 0).await;
    let remaining = get_review(&app, &initial.review.review_id).await;
    assert_eq!(remaining.review.status, ReviewStatus::Open);
    assert!(remaining.review.approved_result_hash.is_none());
    assert_eq!(remaining.drafts.len(), 1);
    assert_eq!(remaining.review.draft_id, approved.drafts[1].draft.draft_id);
    discard(&app, &remaining, 0).await;
    let closed = get_review(&app, &initial.review.review_id).await;
    assert_eq!(closed.review.status, ReviewStatus::Rejected);
}

#[tokio::test]
async fn migration_repairs_stranded_members_without_resurrecting_discarded_content() {
    let (pg, app, project) = fixture().await;
    let first = draft(&app, &project, "first.md").await;
    let second = draft(&app, &project, "second.md").await;
    let initial = review(&app, &[first, second]).await;
    sqlx::query("UPDATE drafts SET status = 'discarded' WHERE draft_id = $1")
        .bind(&initial.drafts[0].draft.draft_id)
        .execute(&pg.pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../migrations/20260920000100_repair_discarded_review_members.sql"
    ))
    .execute(&pg.pool)
    .await
    .unwrap();
    let repaired = get_review(&app, &initial.review.review_id).await;
    assert_eq!(repaired.review.status, ReviewStatus::Open);
    assert_eq!(repaired.drafts.len(), 1);
    assert_eq!(repaired.review.draft_id, initial.drafts[1].draft.draft_id);
    assert!(repaired.review.version > initial.review.version);
    let status: String = sqlx::query_scalar("SELECT status FROM drafts WHERE draft_id = $1")
        .bind(&initial.drafts[0].draft.draft_id)
        .fetch_one(&pg.pool)
        .await
        .unwrap();
    assert_eq!(status, "discarded");
}

#[tokio::test]
async fn conflict_plan_preserves_automatic_sections_and_rejects_foreign_authors_and_stale_heads() {
    let (pg, app, project) = fixture().await;
    let base = "=======\nTitle\n\ncontext\noriginal\ncontext\n\nFooter\n";
    let shared = "=======\nNew title\n\ncontext\nshared\ncontext\n\nFooter\n";
    let proposed = "=======\nTitle\n\ncontext\nproposed\ncontext\n\nNew footer\n";
    let seed = draft_content(&app, &project, "conflict.md", None, base).await;
    let seed = review(&app, &[seed]).await;
    let seed = publish(&app, &seed, None).await;
    let base_head = seed.commit_id.as_deref();
    let proposal = draft_content(&app, &project, "conflict.md", base_head, proposed).await;
    let pending = review(&app, &[proposal]).await;
    let upstream = draft_content(&app, &project, "conflict.md", base_head, shared).await;
    let upstream = review(&app, &[upstream]).await;
    let published = publish(&app, &upstream, base_head).await;
    let head = published.commit_id.as_deref();

    sqlx::query("INSERT INTO users (user_id, email, role, status) VALUES ('usr_other', 'other@example.com', 'member', 'active')")
        .execute(&pg.pool).await.unwrap();
    sqlx::query("INSERT INTO project_members (project_id, user_id, role) VALUES ($1, 'usr_other', 'member')")
        .bind(&project).execute(&pg.pool).await.unwrap();
    let other = common::principal(&pg.pool, "usr_other").await;
    let error = server::app::review::create_review_update_plan(
        &pg.pool,
        &other,
        &pending.review.review_id,
        CreateReviewUpdatePlanRequest {
            expected_review_version: pending.review.version,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(error, server::error::ServerError::Forbidden(_)));

    let plan: ReviewUpdatePlan = post(
        &app,
        &format!("/api/v1/reviews/{}/update-plans", pending.review.review_id),
        CreateReviewUpdatePlanRequest {
            expected_review_version: pending.review.version,
        },
        head,
    )
    .await;
    let candidate = &plan.candidates[0];
    let single: server::app::draft::dto::DraftReconciliationCandidate = post(
        &app,
        &format!(
            "/api/v1/drafts/{}/reconciliation-candidates",
            candidate.draft_id
        ),
        serde_json::json!({"expected_draft_version": candidate.draft_version}),
        head,
    )
    .await;
    assert_eq!(single.merge_preview, candidate.merge_preview);
    let merge = candidate.merge_preview.as_ref().unwrap();
    let text = &merge.state.content.as_ref().unwrap().content;
    assert_eq!(
        merge.marker_length, 8,
        "literal markers in content must remain ordinary text"
    );
    assert!(text.starts_with("=======\nNew title\n"));
    assert!(text.ends_with("\nNew footer\n"));
    assert!(text.contains(
        "<<<<<<<< ours\nshared\n|||||||| original\noriginal\n========\nproposed\n>>>>>>>> theirs\n"
    ));
    let mut resolved = candidate.draft_state.clone();
    resolved.content.as_mut().unwrap().content =
        "=======\nNew title\n\ncontext\ncombined\ncontext\n\nNew footer\n".to_owned();
    let updates = CreateReviewUpdateRequest {
        expected_review_version: plan.detail.review.version,
        drafts: vec![ReviewDraftRequest {
            draft_id: candidate.draft_id.clone(),
            expected_draft_version: candidate.draft_version,
            candidate_id: Some(candidate.candidate_id.clone()),
            resolved_state: Some(resolved),
        }],
    };
    let error = server::app::review::create_review_update(
        &pg.pool,
        &other,
        &pending.review.review_id,
        head,
        updates.clone(),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, server::error::ServerError::Forbidden(_)));
    let (status, _) = request(
        &app,
        "POST",
        &format!("/api/v1/reviews/{}/updates", pending.review.review_id),
        &updates,
        base_head,
    )
    .await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED);
    assert_eq!(
        get_review(&app, &pending.review.review_id).await.drafts,
        plan.detail.drafts
    );
    let updated: ReviewDetail = post(
        &app,
        &format!("/api/v1/reviews/{}/updates", pending.review.review_id),
        updates,
        head,
    )
    .await;
    let merged = publish(&app, &updated, head).await;
    assert_eq!(merged.review.status, ReviewStatus::Merged);
}

#[tokio::test]
async fn automatic_rebase_saves_clean_files_leaves_conflicts_and_survives_reload() {
    use server::app::draft::dto::DraftFreshness;
    let (pg, app, project) = fixture().await;
    let base = "Hotel A\n\nReception\nLuggage\nParking\nPets\nInvoices\n\nBreakfast until 9\n";
    let remote = base.replace("Hotel A", "Hotel B");
    let proposed = base.replace("until 9", "until 10");
    let seed_clean = draft_content(&app, &project, "breakfast.md", None, base).await;
    let seed_conflict =
        draft_content(&app, &project, "checkout.md", None, "Checkout at 12\n").await;
    let seed = review(&app, &[seed_clean, seed_conflict]).await;
    let published = publish(&app, &seed, None).await;
    let base_head = published.commit_id.as_deref();
    let clean = draft_content(&app, &project, "breakfast.md", base_head, &proposed).await;
    let conflict =
        draft_content(&app, &project, "checkout.md", base_head, "Checkout at 14\n").await;
    let pending = review(&app, &[clean, conflict]).await;
    let clean_remote = draft_content(&app, &project, "breakfast.md", base_head, &remote).await;
    let conflicting_remote =
        draft_content(&app, &project, "checkout.md", base_head, "Checkout at 13\n").await;
    let upstream = review(&app, &[clean_remote, conflicting_remote]).await;
    let published = publish(&app, &upstream, base_head).await;
    let head = published.commit_id.as_deref();
    let path = format!("/api/v1/reviews/{}/auto-rebases", pending.review.review_id);
    let (status, _) = request(
        &app,
        "POST",
        &path,
        CreateReviewUpdatePlanRequest {
            expected_review_version: pending.review.version + 1,
        },
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let unchanged = get_review(&app, &pending.review.review_id).await;
    assert_eq!(unchanged.review.version, pending.review.version);
    for (before, after) in pending.drafts.iter().zip(&unchanged.drafts) {
        assert_eq!(before.draft.version, after.draft.version);
        assert_eq!(before.operations, after.operations);
    }
    let plan: ReviewUpdatePlan = post(
        &app,
        &path,
        CreateReviewUpdatePlanRequest {
            expected_review_version: pending.review.version,
        },
        None,
    )
    .await;
    assert_eq!(plan.candidates.len(), 1);
    assert_eq!(
        plan.candidates[0].draft_id,
        pending.drafts[1].draft.draft_id
    );
    let clean = &plan.detail.drafts[0];
    assert_eq!(clean.draft.coordination.freshness, DraftFreshness::Current);
    assert!(clean.draft.coordination.auto_rebased);
    assert_eq!(
        clean.operations[0].input.content.as_ref().unwrap().content,
        remote.replace("until 9", "until 10")
    );
    assert_eq!(
        plan.detail.drafts[1].draft.version,
        pending.drafts[1].draft.version
    );
    assert_eq!(
        plan.detail.drafts[1].operations,
        pending.drafts[1].operations
    );
    assert!(!plan.detail.drafts[1].draft.coordination.auto_rebased);
    assert_eq!(plan.detail.review.status, ReviewStatus::Open);
    assert_eq!(
        get_review(&app, &pending.review.review_id).await,
        plan.detail
    );
    let (status, bytes) = request(&app, "GET", "/api/v1/reviews", (), None).await;
    assert_eq!(status, StatusCode::OK);
    let list: ReviewListResponse = serde_json::from_slice(&bytes).unwrap();
    let listed = list
        .items
        .iter()
        .find(|item| item.review_id == pending.review.review_id)
        .unwrap();
    assert_eq!(listed.coordination, plan.detail.review.coordination);
    let again: ReviewUpdatePlan = post(
        &app,
        &path,
        CreateReviewUpdatePlanRequest {
            expected_review_version: plan.detail.review.version,
        },
        None,
    )
    .await;
    assert_eq!(
        again, plan,
        "a repeat request must not reapply a completed rebase"
    );
    let rebases: i64 = sqlx::query_scalar("SELECT count(*) FROM draft_rebases")
        .fetch_one(&pg.pool)
        .await
        .unwrap();
    assert_eq!(rebases, 1);
    let updates = CreateReviewUpdateRequest {
        expected_review_version: plan.detail.review.version,
        drafts: plan
            .detail
            .drafts
            .iter()
            .map(|item| {
                let candidate = plan
                    .candidates
                    .iter()
                    .find(|candidate| candidate.draft_id == item.draft.draft_id);
                ReviewDraftRequest {
                    draft_id: item.draft.draft_id.clone(),
                    expected_draft_version: item.draft.version,
                    candidate_id: candidate.map(|candidate| candidate.candidate_id.clone()),
                    resolved_state: candidate.map(|candidate| candidate.draft_state.clone()),
                }
            })
            .collect(),
    };
    let resolved: ReviewDetail = post(
        &app,
        &format!("/api/v1/reviews/{}/updates", pending.review.review_id),
        updates,
        head,
    )
    .await;
    assert_eq!(
        resolved.review.coordination.freshness,
        DraftFreshness::Current
    );
    assert_eq!(resolved.review.status, ReviewStatus::Open);
    let merged = publish(&app, &resolved, head).await;
    assert_eq!(merged.review.status, ReviewStatus::Merged);
    assert_eq!(merged.applied_operation_count, 2);
}

#[tokio::test]
async fn automatic_rebase_needs_no_save_and_allows_only_author_or_administrator() {
    let (pg, app, project) = fixture().await;
    let proposal = draft(&app, &project, "breakfast.md").await;
    let pending = review(&app, &[proposal]).await;
    let upstream = draft(&app, &project, "checkout.md").await;
    let upstream = review(&app, &[upstream]).await;
    let published = publish(&app, &upstream, None).await;
    for (id, role) in [("member", "member"), ("reviewer", "admin")] {
        sqlx::query(
            "INSERT INTO users (user_id, email, role, status) VALUES ($1, $2, $3, 'active')",
        )
        .bind(id)
        .bind(format!("{id}@example.com"))
        .bind(role)
        .execute(&pg.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO project_members (project_id, user_id, role) VALUES ($1, $2, 'member')",
        )
        .bind(&project)
        .bind(id)
        .execute(&pg.pool)
        .await
        .unwrap();
    }
    let (member, _) =
        common::authenticated_router_as(pg.pool.clone(), "member@example.com", "member", "Member")
            .await;
    let path = format!("/api/v1/reviews/{}/auto-rebases", pending.review.review_id);
    let body = CreateReviewUpdatePlanRequest {
        expected_review_version: pending.review.version,
    };
    let (status, _) = request(&member, "POST", &path, &body, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        get_review(&app, &pending.review.review_id)
            .await
            .review
            .version,
        pending.review.version
    );
    let (reviewer, _) = common::authenticated_router_as(
        pg.pool.clone(),
        "reviewer@example.com",
        "reviewer",
        "Reviewer",
    )
    .await;
    let plan: ReviewUpdatePlan = post(&reviewer, &path, body, None).await;
    assert!(
        plan.candidates.is_empty(),
        "there is nothing for the user to save"
    );
    assert!(plan.detail.review.coordination.auto_rebased);
    assert_eq!(
        plan.detail.review.status,
        ReviewStatus::Open,
        "automatic rebase must not approve or publish"
    );
    let actor: String =
        sqlx::query_scalar("SELECT applied_by_user_id FROM draft_rebases WHERE draft_id = $1")
            .bind(&pending.draft.draft_id)
            .fetch_one(&pg.pool)
            .await
            .unwrap();
    assert_eq!(
        actor, "reviewer",
        "audit must identify the actual actor, not impersonate the author"
    );
    let merged = publish(&reviewer, &plan.detail, published.commit_id.as_deref()).await;
    assert_eq!(merged.review.status, ReviewStatus::Merged);
}

#[tokio::test]
async fn already_published_changes_auto_rebase_to_no_operations_and_can_be_approved() {
    let (_pg, app, project) = fixture().await;
    let seed = draft_content(&app, &project, "breakfast.md", None, "Breakfast until 9\n").await;
    let seed = review(&app, &[seed]).await;
    let published = publish(&app, &seed, None).await;
    let base = published.commit_id.as_deref();
    let proposal =
        draft_content(&app, &project, "breakfast.md", base, "Breakfast until 10\n").await;
    let pending = review(&app, &[proposal]).await;
    let remote = draft_content(&app, &project, "breakfast.md", base, "Breakfast until 10\n").await;
    let remote = review(&app, &[remote]).await;
    let published = publish(&app, &remote, base).await;
    let plan: ReviewUpdatePlan = post(
        &app,
        &format!("/api/v1/reviews/{}/auto-rebases", pending.review.review_id),
        CreateReviewUpdatePlanRequest {
            expected_review_version: pending.review.version,
        },
        None,
    )
    .await;
    assert!(plan.candidates.is_empty());
    assert!(plan.detail.operations.is_empty());
    assert!(plan.detail.review.coordination.auto_rebased);
    let merged = publish(&app, &plan.detail, published.commit_id.as_deref()).await;
    assert_eq!(merged.review.status, ReviewStatus::Merged);
    assert_eq!(merged.applied_operation_count, 0);
}
