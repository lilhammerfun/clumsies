//! Project publication permissions, isolation, and explicit Organization adaptations.

mod common;

use draft::dto::{
    CreateDraftRequest, DraftOperationAction, DraftOperationInput, DraftResourceContent,
    DraftResourceRef,
};
use memory::dto::{OrgMemorySource, ResourceScope};
use review::dto::{CreateReviewMergeRequest, CreateReviewRequest, Review, ReviewDraftRequest};
use server::app::{auth::AuthPrincipal, draft, memory, review};
use sqlx::PgPool;

async fn head(pool: &PgPool, scope: &str, project: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT commit_id FROM refs WHERE scope = $1 AND ($1 = 'org' OR project_id = $2)",
    )
    .bind(scope)
    .bind(project)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
async fn proposal(
    pool: &PgPool,
    author: &AuthPrincipal,
    project: &str,
    scope: ResourceScope,
    action: DraftOperationAction,
    id: Option<&str>,
    path: &str,
    text: &str,
    origin: Option<OrgMemorySource>,
) -> Review {
    let base = head(pool, scope.as_str(), project).await;
    let resource = DraftResourceRef {
        scope,
        id: id.map(str::to_owned),
        path: Some(path.to_owned()),
    };
    let detail = draft::create_draft(
        pool,
        author,
        CreateDraftRequest {
            daemon_installation_id: "ownership-test".to_owned(),
            project_id: project.to_owned(),
            base_commit_id: base.clone(),
            title: path.to_owned(),
            description: None,
            resource: resource.clone(),
            operations: vec![DraftOperationInput {
                action,
                resource,
                new_path: None,
                content: (action != DraftOperationAction::Delete).then(|| DraftResourceContent {
                    org_source: origin,
                    description: None,
                    content: text.to_owned(),
                }),
            }],
        },
    )
    .await
    .unwrap();
    review::create_review(
        pool,
        author,
        base.as_deref(),
        CreateReviewRequest {
            org_contribution: None,
            title: None,
            description: None,
            drafts: vec![ReviewDraftRequest {
                draft_id: detail.draft.draft_id,
                expected_draft_version: detail.draft.version,
                candidate_id: None,
                resolved_state: None,
            }],
        },
    )
    .await
    .unwrap()
    .review
}

/// Publish the current Review with its current target reference.
///
/// # Errors
/// Returns authorization, version, or publication failures for assertions.
async fn merge(
    pool: &PgPool,
    actor: &AuthPrincipal,
    item: &Review,
) -> Result<review::dto::ReviewMergeResult, server::error::ServerError> {
    review::create_review_merge(
        pool,
        &item.review_id,
        actor,
        head(pool, item.scope.as_str(), &item.project_id)
            .await
            .as_deref(),
        CreateReviewMergeRequest {
            expected_review_version: item.version,
        },
    )
    .await
}

#[tokio::test]
async fn project_maintainer_publishes_adaptation_without_org_authority() {
    let postgres = common::migrated_postgres().await;
    let pool = &postgres.pool;
    let installation = common::initialize_installation(
        pool.clone(),
        "Ownership",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Payments",
    )
    .await;
    let owner = common::owner_principal(pool).await;
    let project = &installation.project_id;
    for (id, role) in [("usr_member", "member"), ("usr_maintainer", "admin")] {
        sqlx::query(
            "INSERT INTO users (user_id, email, role, status) VALUES ($1, $2, 'member', 'active')",
        )
        .bind(id)
        .bind(format!("{id}@example.com"))
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO project_members (project_id, user_id, role) VALUES ($1, $2, $3)")
            .bind(project)
            .bind(id)
            .bind(role)
            .execute(pool)
            .await
            .unwrap();
    }
    let member = common::principal(pool, "usr_member").await;
    let maintainer = common::principal(pool, "usr_maintainer").await;
    let org_id = memory::create_org_context(pool, &owner, "runbook.md", "Org baseline")
        .await
        .unwrap();
    memory::select_org_resource_for_project(pool, &owner, project, &org_id)
        .await
        .unwrap();
    let source_commit = head(pool, "project", project).await.unwrap();
    let org_before = head(pool, "org", project).await;
    let origin = OrgMemorySource {
        resource_id: org_id.clone(),
        commit_id: source_commit,
    };
    let forged_resource = DraftResourceRef {
        scope: ResourceScope::Project,
        id: None,
        path: Some("forged.md".to_owned()),
    };
    let forged = draft::create_draft(
        pool,
        &member,
        CreateDraftRequest {
            daemon_installation_id: "ownership-test".to_owned(),
            project_id: project.clone(),
            base_commit_id: Some(origin.commit_id.clone()),
            title: "Forged origin".to_owned(),
            description: None,
            resource: forged_resource.clone(),
            operations: vec![DraftOperationInput {
                action: DraftOperationAction::Create,
                resource: forged_resource,
                new_path: None,
                content: Some(DraftResourceContent {
                    description: None,
                    content: "Invalid origin".to_owned(),
                    org_source: Some(OrgMemorySource {
                        resource_id: org_id.clone(),
                        commit_id: org_before.clone().unwrap(),
                    }),
                }),
            }],
        },
    )
    .await;
    assert!(
        forged.is_err(),
        "an Org commit cannot impersonate the selected Project snapshot"
    );
    let item = proposal(
        pool,
        &member,
        project,
        ResourceScope::Project,
        DraftOperationAction::Create,
        None,
        "runbook.md",
        "Payments adaptation",
        Some(origin.clone()),
    )
    .await;
    assert!(
        merge(pool, &member, &item).await.is_err(),
        "a member cannot publish"
    );
    let result = merge(pool, &maintainer, &item).await.unwrap();
    assert_eq!(head(pool, "org", project).await, org_before);
    assert_eq!(head(pool, "project", project).await, result.commit_id);
    let adapted_id: String = sqlx::query_scalar("SELECT resource_id FROM resources WHERE scope = 'project' AND project_id = $1 AND status = 'active'")
        .bind(project).fetch_one(pool).await.unwrap();
    assert_ne!(adapted_id, org_id);
    let entries: Vec<(String, String)> = sqlx::query_as("SELECT e.item_id, b.content FROM refs r JOIN commits c USING(commit_id) JOIN tree_entries e USING(tree_id) JOIN blobs b USING(blob_id) WHERE r.project_id = $1 AND e.resource_kind = 'memory'")
        .bind(project).fetch_all(pool).await.unwrap();
    assert_eq!(
        entries,
        vec![(adapted_id.clone(), "Payments adaptation".to_owned())]
    );
    let exported = memory::export_memory_state(pool, &owner).await.unwrap();
    assert_eq!(
        exported
            .memories
            .iter()
            .find(|item| item.memory_id == adapted_id)
            .unwrap()
            .org_source,
        Some(origin.clone())
    );
    let notified: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inbox_notifications WHERE user_id = 'usr_member' AND project_id = $1 AND kind = 'shared_update' AND event_key = $2)")
        .bind(project).bind(&result.commit_id).fetch_one(pool).await.unwrap();
    assert!(notified);
    // Org publication is independently authorized and never overwrites the adaptation.
    for (action, body) in [
        (DraftOperationAction::Update, "Org v2"),
        (DraftOperationAction::Delete, ""),
    ] {
        let org_review = proposal(
            pool,
            &member,
            project,
            ResourceScope::Org,
            action,
            Some(&org_id),
            "runbook.md",
            body,
            None,
        )
        .await;
        assert!(
            merge(pool, &maintainer, &org_review).await.is_err(),
            "project admin is not org admin"
        );
        merge(pool, &owner, &org_review).await.unwrap();
        let adapted: (String, serde_json::Value) = sqlx::query_as(
            "SELECT body, org_source FROM resources WHERE resource_id = $1 AND status = 'active'",
        )
        .bind(&adapted_id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(adapted.0, "Payments adaptation");
        assert_eq!(adapted.1, serde_json::to_value(&origin).unwrap());
        let content: String = sqlx::query_scalar("SELECT b.content FROM refs r JOIN commits c USING(commit_id) JOIN tree_entries e USING(tree_id) JOIN blobs b USING(blob_id) WHERE r.project_id = $1 AND e.item_id = $2")
            .bind(project).bind(&adapted_id).fetch_one(pool).await.unwrap();
        assert_eq!(content, "Payments adaptation");
    }
    let migration = server::maintenance::project_authority::migrate_project_authority(
        pool,
        server::maintenance::project_authority::MigrationMode::Apply {
            expected_plan_hash: "old-plan",
        },
    )
    .await;
    assert!(migration.unwrap_err().to_string().contains("retired"));
    postgres.shutdown().await;
}

#[tokio::test]
async fn contribution_retries_use_one_fixed_project_publication_and_independent_review() {
    let postgres = common::migrated_postgres().await;
    let pool = &postgres.pool;
    let installation = common::initialize_installation(
        pool.clone(),
        "Contributions",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Payments",
    )
    .await;
    let owner = common::owner_principal(pool).await;
    let project = &installation.project_id;
    let base = head(pool, "project", project).await;
    let resource = DraftResourceRef {
        scope: ResourceScope::Project,
        id: None,
        path: Some("payments.md".to_owned()),
    };
    let detail = draft::create_draft(
        pool,
        &owner,
        CreateDraftRequest {
            daemon_installation_id: "contribution-test".to_owned(),
            project_id: project.clone(),
            base_commit_id: base.clone(),
            title: "Publish payment checklist".to_owned(),
            description: None,
            resource: resource.clone(),
            operations: vec![DraftOperationInput {
                action: DraftOperationAction::Create,
                resource,
                new_path: None,
                content: Some(DraftResourceContent {
                    org_source: None,
                    description: None,
                    content: "Reviewed Project v1".to_owned(),
                }),
            }],
        },
    )
    .await
    .unwrap();
    let item = review::create_review(
        pool,
        &owner,
        base.as_deref(),
        CreateReviewRequest {
            org_contribution: Some(vec![review::dto::OrgContributionEntry {
                draft_id: detail.draft.draft_id.clone(),
                target_id: None,
                path: Some("shared-payments.md".to_owned()),
            }]),
            title: None,
            description: None,
            drafts: vec![ReviewDraftRequest {
                draft_id: detail.draft.draft_id,
                expected_draft_version: 1,
                candidate_id: None,
                resolved_state: None,
            }],
        },
    )
    .await
    .unwrap()
    .review;
    review::create_review_decision(
        pool,
        &item.review_id,
        &owner,
        review::dto::CreateReviewDecisionRequest {
            decision: review::dto::ReviewDecision::Rejected,
            expected_review_version: item.version,
            body: None,
        },
    )
    .await
    .unwrap();
    let reopened = draft::get_draft(pool, &owner, &item.draft_id)
        .await
        .unwrap();
    let item = review::create_review(
        pool,
        &owner,
        base.as_deref(),
        CreateReviewRequest {
            org_contribution: Some(vec![review::dto::OrgContributionEntry {
                draft_id: item.draft_id.clone(),
                target_id: None,
                path: Some("shared/resubmitted-payments.md".to_owned()),
            }]),
            drafts: vec![ReviewDraftRequest {
                draft_id: item.draft_id.clone(),
                expected_draft_version: reopened.draft.version,
                candidate_id: None,
                resolved_state: None,
            }],
            title: None,
            description: None,
        },
    )
    .await
    .unwrap()
    .review;
    assert_eq!(
        item.org_contribution.as_ref().unwrap().entries[0]
            .path
            .as_deref(),
        Some("shared/resubmitted-payments.md")
    );
    // Failure at the external boundary must not roll back successful Project publication.
    sqlx::raw_sql("CREATE FUNCTION fail_contribution() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
        IF NEW.daemon_installation_id = 'server-org-contribution' THEN RAISE EXCEPTION 'temporary contribution failure'; END IF;
        RETURN NEW; END $$;
        CREATE TRIGGER fail_contribution BEFORE INSERT ON drafts FOR EACH ROW EXECUTE FUNCTION fail_contribution();")
        .execute(pool).await.unwrap();
    let published = merge(pool, &owner, &item).await.unwrap();
    let failed = published.review.org_contribution.as_ref().unwrap();
    assert!(
        failed
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("Retry when the service"))
    );
    assert!(failed.org_review_id.is_none());
    assert_eq!(failed.source_commit_id, published.commit_id);
    let original_commit = published.commit_id;
    sqlx::raw_sql("DROP TRIGGER fail_contribution ON drafts; DROP FUNCTION fail_contribution();")
        .execute(pool)
        .await
        .unwrap();
    let memory_id: String = sqlx::query_scalar(
        "SELECT resource_id FROM resources WHERE scope = 'project' AND project_id = $1",
    )
    .bind(project)
    .fetch_one(pool)
    .await
    .unwrap();
    let newer = proposal(
        pool,
        &owner,
        project,
        ResourceScope::Project,
        DraftOperationAction::Update,
        Some(&memory_id),
        "payments.md",
        "Newer Project v2",
        None,
    )
    .await;
    merge(pool, &owner, &newer).await.unwrap();
    let project_head = head(pool, "project", project).await;
    let org_head = head(pool, "org", project).await;
    let (retry_a, retry_b) = tokio::join!(
        review::retry_org_contribution(pool, &owner, &item.review_id),
        review::retry_org_contribution(pool, &owner, &item.review_id)
    );
    let contribution = retry_a.unwrap().review.org_contribution.unwrap();
    assert_eq!(
        contribution.org_review_id,
        retry_b
            .unwrap()
            .review
            .org_contribution
            .unwrap()
            .org_review_id
    );
    assert_eq!(contribution.source_commit_id, original_commit);
    assert!(contribution.last_error.is_none());
    let linked =
        review::get_review_detail(pool, &owner, contribution.org_review_id.as_deref().unwrap())
            .await
            .unwrap();
    assert_eq!(linked.review.scope, ResourceScope::Org);
    assert_eq!(
        linked.review.project_source.as_ref().unwrap().commit_id,
        original_commit.unwrap()
    );
    assert_eq!(
        linked.operations[0].input.content.as_ref().unwrap().content,
        "Reviewed Project v1"
    );
    review::create_review_decision(
        pool,
        &linked.review.review_id,
        &owner,
        review::dto::CreateReviewDecisionRequest {
            decision: review::dto::ReviewDecision::Rejected,
            expected_review_version: linked.review.version,
            body: Some("Needs generalization".to_owned()),
        },
    )
    .await
    .unwrap();
    assert_eq!(head(pool, "project", project).await, project_head);
    assert_eq!(head(pool, "org", project).await, org_head);
    let retry = review::retry_org_contribution(pool, &owner, &item.review_id)
        .await
        .unwrap();
    assert_eq!(
        retry.review.org_contribution.unwrap().org_review_id,
        contribution.org_review_id
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM drafts WHERE daemon_installation_id = 'server-org-contribution'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    postgres.shutdown().await;
}

#[tokio::test]
async fn automatic_project_reconciliation_preserves_conflicts_and_invalidates_changed_approval() {
    let postgres = common::migrated_postgres().await;
    let pool = &postgres.pool;
    let installation = common::initialize_installation(
        pool.clone(),
        "Reconciliation",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Payments",
    )
    .await;
    let owner = common::owner_principal(pool).await;
    let project = &installation.project_id;
    let initial = proposal(
        pool,
        &owner,
        project,
        ResourceScope::Project,
        DraftOperationAction::Create,
        None,
        "runbook.md",
        "one\ntwo\nthree\nfour\nfive\n",
        None,
    )
    .await;
    merge(pool, &owner, &initial).await.unwrap();
    let id: String =
        sqlx::query_scalar("SELECT resource_id FROM resources WHERE scope = 'project'")
            .fetch_one(pool)
            .await
            .unwrap();
    let clean = proposal(
        pool,
        &owner,
        project,
        ResourceScope::Project,
        DraftOperationAction::Update,
        Some(&id),
        "runbook.md",
        "ONE\ntwo\nthree\nfour\nfive\n",
        None,
    )
    .await;
    let conflict = proposal(
        pool,
        &owner,
        project,
        ResourceScope::Project,
        DraftOperationAction::Update,
        Some(&id),
        "runbook.md",
        "one\ntwo\nthree\nfour\nMY FIVE\n",
        None,
    )
    .await;
    review::create_review_decision(
        pool,
        &clean.review_id,
        &owner,
        review::dto::CreateReviewDecisionRequest {
            decision: review::dto::ReviewDecision::Approved,
            expected_review_version: clean.version,
            body: None,
        },
    )
    .await
    .unwrap();
    let upstream = proposal(
        pool,
        &owner,
        project,
        ResourceScope::Project,
        DraftOperationAction::Update,
        Some(&id),
        "runbook.md",
        "one\ntwo\nthree\nfour\nREMOTE FIVE\n",
        None,
    )
    .await;
    merge(pool, &owner, &upstream).await.unwrap();
    for (item, is_clean) in [(&clean, true), (&conflict, false)] {
        let before = draft::get_draft(pool, &owner, &item.draft_id)
            .await
            .unwrap();
        let after = draft::auto_rebase_draft(
            pool,
            &owner,
            &item.draft_id,
            draft::dto::CreateDraftReconciliationCandidateRequest {
                expected_draft_version: before.draft.version,
            },
        )
        .await
        .unwrap();
        if is_clean {
            assert_eq!(
                after.draft.coordination.freshness,
                draft::dto::DraftFreshness::Current
            );
            assert!(after.draft.coordination.auto_rebased);
            let updated = review::get_review(pool, &owner, &item.review_id)
                .await
                .unwrap();
            assert_eq!(updated.status, review::dto::ReviewStatus::Open);
            assert!(updated.approved_result_hash.is_none());
        } else {
            assert_eq!(after.draft.base_commit_id, before.draft.base_commit_id);
            assert_eq!(after.draft.version, before.draft.version);
            assert_eq!(after.operations, before.operations);
            assert_eq!(
                after.draft.coordination.reconciliation,
                draft::dto::DraftReconciliationStatus::Conflicts
            );
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM inbox_notifications WHERE kind = 'draft_conflict' AND target_id = $1 AND user_id = $2")
                .bind(&item.draft_id).bind(&owner.user_id).fetch_one(pool).await.unwrap();
            assert_eq!(count, 1);
        }
    }
    postgres.shutdown().await;
}
