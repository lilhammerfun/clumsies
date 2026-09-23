//! Durable links from a fixed Project publication to an independently reviewed Org proposal.

use super::{dto::*, repository, service};
use crate::app::auth::AuthPrincipal;
use crate::app::commit::current_org_ref;
use crate::app::draft::{
    self,
    dto::{
        CreateDraftRequest, DraftOperationAction, DraftOperationInput, DraftResourceContent,
        DraftResourceRef,
    },
};
use crate::app::memory::{
    dto::ResourceScope, lock_org_draft_selection_coordination_for_project, project_org_id,
};
use crate::error::ServerError;
use sqlx::{PgPool, Postgres, Transaction};
use std::collections::BTreeSet;

/// Validate explicit contribution choices before accepting the Project submission.
///
/// # Errors
/// Rejects mixed ownership, empty or duplicate choices, foreign drafts, and deleted results.
pub(super) async fn record_intent(
    tx: &mut Transaction<'_, Postgres>,
    review_id: &str,
    scope: ResourceScope,
    drafts: &[ReviewDraftRequest],
    entries: Vec<OrgContributionEntry>,
) -> Result<(), ServerError> {
    if scope != ResourceScope::Project || entries.is_empty() {
        return Err(ServerError::InvalidRequest(
            "an Organization contribution must select Project Review results".to_owned(),
        ));
    }
    let included: BTreeSet<_> = drafts.iter().map(|draft| &draft.draft_id).collect();
    let mut sources = BTreeSet::new();
    let mut targets = BTreeSet::new();
    for entry in &entries {
        if !included.contains(&entry.draft_id)
            || !sources.insert(&entry.draft_id)
            || entry
                .target_id
                .as_ref()
                .is_some_and(|id| !targets.insert(id))
        {
            return Err(ServerError::InvalidRequest("contribution entries must select distinct included drafts and Organization targets".to_owned()));
        }
        if !draft::draft_result_state(tx, &entry.draft_id).await?.exists {
            return Err(ServerError::InvalidRequest(
                "a deleted Project result cannot be contributed".to_owned(),
            ));
        }
        if let Some(path) = &entry.path {
            crate::app::memory::model::validate_resource_path(path)?;
        }
        if entry.path.is_some() && entry.target_id.is_some() {
            return Err(ServerError::InvalidRequest(
                "an existing Organization target keeps its own path".to_owned(),
            ));
        }
    }
    repository::insert_contribution(tx, review_id, &entries).await
}

/// Retry a pending intent; publication and retries never share a transaction.
///
/// # Errors
/// Rejects inaccessible reviews or unauthorized actors and propagates persistence failures.
pub async fn retry_org_contribution(
    pool: &PgPool,
    principal: &AuthPrincipal,
    review_id: &str,
) -> Result<ReviewDetail, ServerError> {
    let detail = service::get_review_detail(pool, principal, review_id).await?;
    if detail.review.author.user_id != principal.user_id {
        let mut tx = pool.begin().await?;
        service::ensure_review_admin(&mut tx, principal, review_id).await?;
        tx.commit().await?;
    }
    if detail.review.status != ReviewStatus::Merged || detail.review.org_contribution.is_none() {
        return Err(ServerError::InvalidRequest(
            "only a merged Project Review with a contribution intent can be retried".to_owned(),
        ));
    }
    create_after_merge(pool, review_id).await?;
    service::get_review_detail(pool, principal, review_id).await
}

/// Attempt independent creation and persist failures for an explicit idempotent retry.
///
/// # Errors
/// Returns only failures to record the outcome; creation errors stay on the source Review.
pub(super) async fn create_after_merge(pool: &PgPool, review_id: &str) -> Result<(), ServerError> {
    if let Err(error) = create_linked_review(pool, review_id).await {
        let message = match error {
            ServerError::Sqlx(_) => "Organization contribution could not be created. Retry when the service is available.".to_owned(),
            other => other.to_string(),
        };
        repository::record_contribution_error(pool, review_id, &message).await?;
    }
    Ok(())
}

/// Copy selected fixed snapshot content into ordinary Org Drafts and one ordinary Org Review.
///
/// # Errors
/// Rejects lost access, absent fixed content, invalid targets, and persistence failures atomically.
async fn create_linked_review(pool: &PgPool, review_id: &str) -> Result<(), ServerError> {
    let mut tx = pool.begin().await?;
    let Some(intent) = repository::lock_contribution(&mut tx, review_id).await? else {
        return Ok(());
    };
    if intent.org_review_id.is_some() {
        return Ok(());
    }
    let Some(source_commit) = intent.source_commit_id else {
        return Ok(());
    };
    let source = service::load_review(&mut tx, review_id).await?;
    if source.scope != ResourceScope::Project || source.status != ReviewStatus::Merged {
        return Err(ServerError::InvalidRequest(
            "contribution requires a published Project Review".to_owned(),
        ));
    }
    let project = &source.project_id;
    let author = &source.author.user_id;
    repository::ensure_contribution_author(&mut tx, project, author).await?;
    lock_org_draft_selection_coordination_for_project(&mut tx, project).await?;
    let org = project_org_id(&mut tx, project).await?;
    let org_base = current_org_ref(&mut tx, &org).await?;
    let mut drafts = Vec::new();
    let published_drafts = repository::load_review_draft_ids(&mut tx, review_id).await?;
    for entry in intent.entries {
        if !published_drafts.contains(&entry.draft_id) {
            return Err(ServerError::InvalidRequest(
                "a contribution source was removed from the Project Review".to_owned(),
            ));
        }
        let state = draft::draft_result_state(&mut tx, &entry.draft_id).await?;
        let source_path = state.resource.path.ok_or_else(|| {
            ServerError::InvalidRequest("published result has no path".to_owned())
        })?;
        let body =
            repository::contribution_source_content(&mut tx, project, &source_commit, &source_path)
                .await?;
        let resource = DraftResourceRef {
            scope: ResourceScope::Org,
            id: entry.target_id,
            path: Some(entry.path.unwrap_or(source_path)),
        };
        let action = if resource.id.is_some() {
            DraftOperationAction::Update
        } else {
            DraftOperationAction::Create
        };
        let draft_id = draft::create_draft_in_tx(
            &mut tx,
            author,
            CreateDraftRequest {
                daemon_installation_id: "server-org-contribution".to_owned(),
                project_id: project.clone(),
                base_commit_id: org_base.clone(),
                title: source.title.clone(),
                description: Some(source.description.clone()),
                resource: resource.clone(),
                operations: vec![DraftOperationInput {
                    action,
                    resource,
                    content: Some(DraftResourceContent {
                        org_source: None,
                        description: None,
                        content: body,
                    }),
                    new_path: None,
                }],
            },
        )
        .await?;
        drafts.push(ReviewDraftRequest {
            draft_id,
            expected_draft_version: 1,
            candidate_id: None,
            resolved_state: None,
        });
    }
    let detail = service::create_review_in_tx(
        &mut tx,
        author,
        org_base.as_deref(),
        CreateReviewRequest {
            org_contribution: None,
            drafts,
            title: Some(source.title),
            description: Some(source.description),
        },
    )
    .await?
    .into_result()?;
    repository::finish_contribution(&mut tx, review_id, &detail.review.review_id).await?;
    tx.commit().await?;
    Ok(())
}
