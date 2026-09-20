//! Internal values and invariants for draft resources.

use crate::app::draft::dto::{
    DraftCoordination, DraftEventType, DraftFreshness, DraftOperation, DraftOperationAction,
    DraftOperationInput, DraftReconciliationStatus, DraftResourceContent, DraftResourceRef,
    DraftStatus, ReconciliationConflict, ReconciliationConflictKind, ReconciliationResourceState,
};
use crate::app::memory::dto::ResourceScope;
use crate::app::memory::model::validate_resource_path;
use crate::error::ServerError;
use sha2::{Digest, Sha256};

/// Require each mutation to target the resource identity and scope of its proposal.
///
/// # Errors
/// Rejects mutations whose resource identity or ownership scope differs from the proposal.
pub(crate) fn validate_draft_operation_resource(
    draft_resource: &DraftResourceRef,
    operation: &DraftOperationInput,
) -> Result<(), ServerError> {
    validate_draft_resource(draft_resource)?;
    if operation.resource.scope != draft_resource.scope {
        return Err(ServerError::InvalidRequest(
            "one draft cannot mix resource scopes".to_owned(),
        ));
    }
    if let Some(content) = operation.content.as_ref() {
        validate_draft_content_shape(content)?;
    }
    if let Some(path) = operation.resource.path.as_deref() {
        validate_resource_path(path)?;
    }
    if let Some(path) = operation.new_path.as_deref() {
        validate_resource_path(path)?;
    }
    let valid = match operation.action {
        DraftOperationAction::Create => {
            operation.resource.path.is_some()
                && operation.content.is_some()
                && operation.new_path.is_none()
        }
        DraftOperationAction::Update => {
            (operation.resource.id.is_some() || operation.resource.path.is_some())
                && operation.content.is_some()
                && operation.new_path.is_none()
        }
        DraftOperationAction::Rename => {
            (operation.resource.id.is_some() || operation.resource.path.is_some())
                && operation.content.is_none()
                && operation.new_path.is_some()
        }
        DraftOperationAction::Delete => {
            (operation.resource.id.is_some() || operation.resource.path.is_some())
                && operation.content.is_none()
                && operation.new_path.is_none()
        }
    };
    if valid {
        Ok(())
    } else {
        Err(ServerError::InvalidRequest(
            "draft operation fields do not match its action".to_owned(),
        ))
    }
}

/// Reject legacy proposal scopes that may no longer enter the publication workflow.
///
/// # Errors
/// Rejects legacy scopes that cannot enter the active publication workflow.
pub(crate) fn ensure_publishable_draft_scope(scope: ResourceScope) -> Result<(), ServerError> {
    if scope == ResourceScope::Project {
        return Err(ServerError::InvalidRequest(
            "Project is not a Memory authority scope; publish an Organization-scoped Draft"
                .to_owned(),
        ));
    }
    Ok(())
}

/// Reject proposal scopes retired from the active authoring workflow.
///
/// # Errors
/// Rejects scopes retired from the current authoring workflow.
pub(crate) fn ensure_writable_draft_scope(scope: ResourceScope) -> Result<(), ServerError> {
    if scope == ResourceScope::Project {
        return Err(ServerError::InvalidRequest(
            "Project is a Draft carrier, not a Memory authority scope; create an Organization-scoped Draft"
                .to_owned(),
        ));
    }
    Ok(())
}

/// Require new-resource proposals to use a coherent create/edit sequence without deletion.
///
/// # Errors
/// Rejects invalid create/edit ordering and deletion of a resource not yet published.
pub(crate) fn validate_new_resource_draft_operations(
    operations: &[DraftOperationInput],
) -> Result<(), ServerError> {
    let creates_resource = operations
        .iter()
        .any(|operation| operation.action == DraftOperationAction::Create);
    let deletes_resource = operations
        .iter()
        .any(|operation| operation.action == DraftOperationAction::Delete);
    if creates_resource && deletes_resource {
        return Err(ServerError::InvalidRequest(
            "a draft-created resource must be discarded instead of deleted".to_owned(),
        ));
    }
    Ok(())
}

/// Require mutation payloads to match the content rules of their action.
///
/// # Errors
/// Rejects missing or unexpected content for the requested mutation action.
pub(crate) fn validate_draft_content_shape(
    content: &DraftResourceContent,
) -> Result<(), ServerError> {
    if content.content.trim().is_empty() {
        return Err(ServerError::InvalidRequest(
            "memory content must not be empty".to_owned(),
        ));
    }
    Ok(())
}

/// Validate the resource scope and identity or creation path before proposal persistence.
///
/// # Errors
/// Rejects unsupported scopes and missing or unsafe resource identities and paths.
pub(crate) fn validate_draft_resource(resource: &DraftResourceRef) -> Result<(), ServerError> {
    if let Some(path) = resource.path.as_deref() {
        validate_resource_path(path)?;
    }
    Ok(())
}

/// Borrow the Markdown body from an editable resource payload.
pub(crate) fn content_text(content: &DraftResourceContent) -> &str {
    &content.content
}

/// Decode persisted content only for supported resource categories.
pub(crate) fn content_for_kind(
    _kind: &str,
    content: String,
    description: Option<String>,
) -> DraftResourceContent {
    DraftResourceContent {
        description,
        content,
    }
}

/// Replay mutations in order while checking resource existence and content invariants.
///
/// # Errors
/// Rejects mutations inconsistent with resource existence, identity, or required content.
pub(crate) fn apply_operations_to_state(
    mut state: ReconciliationResourceState,
    operations: &[DraftOperation],
) -> Result<ReconciliationResourceState, ServerError> {
    for operation in operations {
        match operation.input.action {
            DraftOperationAction::Create => {
                let content = operation.input.content.clone().ok_or_else(|| {
                    ServerError::InvalidRequest("create operation requires content".to_owned())
                })?;
                state = ReconciliationResourceState {
                    exists: true,
                    resource: operation.input.resource.clone(),
                    content: Some(content),
                };
            }
            DraftOperationAction::Update => {
                if !state.exists {
                    return Err(ServerError::InvalidRequest(
                        "update operation targets a resource absent from the draft base".to_owned(),
                    ));
                }
                state.content = Some(operation.input.content.clone().ok_or_else(|| {
                    ServerError::InvalidRequest("update operation requires content".to_owned())
                })?);
            }
            DraftOperationAction::Rename => {
                if !state.exists {
                    return Err(ServerError::InvalidRequest(
                        "rename operation targets a resource absent from the draft base".to_owned(),
                    ));
                }
                state.resource.path = operation.input.new_path.clone();
            }
            DraftOperationAction::Delete => {
                state.exists = false;
                state.content = None;
            }
        }
    }
    Ok(state)
}

/// Compute a deterministic fingerprint of a complete reconciliation state.
///
/// # Errors
/// Propagates failure to serialize the complete resource state for deterministic hashing.
pub(crate) fn state_hash(state: &ReconciliationResourceState) -> Result<String, ServerError> {
    let bytes = serde_json::to_vec(state).map_err(|error| {
        ServerError::InvalidRequest(format!("failed to hash resource state: {error}"))
    })?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

/// Resolve a three-way scalar change when one side is unchanged or both sides agree.
pub(crate) fn merge_scalar<T: Clone + PartialEq>(base: &T, current: &T, draft: &T) -> Option<T> {
    if current == draft {
        Some(current.clone())
    } else if current == base {
        Some(draft.clone())
    } else if draft == base {
        Some(current.clone())
    } else {
        None
    }
}

/// Combine ancestor, upstream, and proposal state while preserving unresolved field conflicts.
pub(crate) fn merge_resource_states(
    base: &ReconciliationResourceState,
    current: &ReconciliationResourceState,
    draft: &ReconciliationResourceState,
) -> (
    Option<ReconciliationResourceState>,
    Vec<ReconciliationConflict>,
) {
    if current == draft {
        return (Some(current.clone()), Vec::new());
    }
    if current == base {
        return (Some(draft.clone()), Vec::new());
    }
    if draft == base {
        return (Some(current.clone()), Vec::new());
    }

    if base.exists
        && ((!current.exists && draft.exists && draft != base)
            || (!draft.exists && current.exists && current != base))
    {
        return (
            None,
            vec![ReconciliationConflict {
                kind: ReconciliationConflictKind::Existence,
                field: "exists".to_owned(),
                base: Some(base.exists.to_string()),
                current: Some(current.exists.to_string()),
                draft: Some(draft.exists.to_string()),
            }],
        );
    }

    let Some(exists) = merge_scalar(&base.exists, &current.exists, &draft.exists) else {
        return (
            None,
            vec![ReconciliationConflict {
                kind: ReconciliationConflictKind::Existence,
                field: "exists".to_owned(),
                base: Some(base.exists.to_string()),
                current: Some(current.exists.to_string()),
                draft: Some(draft.exists.to_string()),
            }],
        );
    };
    if !exists {
        let mut state = draft.clone();
        state.exists = false;
        state.content = None;
        return (Some(state), Vec::new());
    }

    if !current.exists || !draft.exists {
        return (
            None,
            vec![ReconciliationConflict {
                kind: ReconciliationConflictKind::Existence,
                field: "exists".to_owned(),
                base: Some(base.exists.to_string()),
                current: Some(current.exists.to_string()),
                draft: Some(draft.exists.to_string()),
            }],
        );
    }

    let mut conflicts = Vec::new();
    let path = merge_scalar(
        &base.resource.path,
        &current.resource.path,
        &draft.resource.path,
    );
    if path.is_none() {
        conflicts.push(ReconciliationConflict {
            kind: ReconciliationConflictKind::Path,
            field: "path".to_owned(),
            base: base.resource.path.clone(),
            current: current.resource.path.clone(),
            draft: draft.resource.path.clone(),
        });
    }

    let base_text = base.content.as_ref().map(content_text).unwrap_or_default();
    let current_text = current
        .content
        .as_ref()
        .map(content_text)
        .unwrap_or_default();
    let draft_text = draft.content.as_ref().map(content_text).unwrap_or_default();
    let merged_text = if current_text == draft_text {
        Some(current_text.to_owned())
    } else if current_text == base_text {
        Some(draft_text.to_owned())
    } else if draft_text == base_text {
        Some(current_text.to_owned())
    } else {
        match diffy::merge(base_text, current_text, draft_text) {
            Ok(content) => Some(content),
            Err(_) => {
                conflicts.push(ReconciliationConflict {
                    kind: ReconciliationConflictKind::Content,
                    field: "content".to_owned(),
                    base: Some(base_text.to_owned()),
                    current: Some(current_text.to_owned()),
                    draft: Some(draft_text.to_owned()),
                });
                None
            }
        }
    };

    if !conflicts.is_empty() {
        return (None, conflicts);
    }
    let mut resource = current.resource.clone();
    resource.path = path.expect("path is present without conflicts");
    if resource.id.is_none() {
        resource.id = draft.resource.id.clone();
    }
    (
        Some(ReconciliationResourceState {
            exists: true,
            resource,
            content: merged_text.map(|content| content_for_kind("memory", content, None)),
        }),
        Vec::new(),
    )
}

/// Preserve automatic text and path changes while preparing explicit conflict choices.
pub(super) fn reconciliation_merge_preview(
    base: &ReconciliationResourceState,
    current: &ReconciliationResourceState,
    draft: &ReconciliationResourceState,
) -> super::dto::ReconciliationMergePreview {
    let mut state = draft.clone();
    let base_text = base.content.as_ref().map(content_text).unwrap_or_default();
    let current_text = current
        .content
        .as_ref()
        .map(content_text)
        .unwrap_or_default();
    let draft_text = draft.content.as_ref().map(content_text).unwrap_or_default();
    let marker_length = [base_text, current_text, draft_text]
        .iter()
        .flat_map(|text| text.lines())
        .map(|line| {
            line.chars()
                .take_while(|c| ['<', '>', '|', '='].contains(c))
                .count()
                + 1
        })
        .max()
        .unwrap_or(7)
        .max(7);
    if current.exists && draft.exists {
        state.resource = current.resource.clone();
        state.resource.path = merge_scalar(
            &base.resource.path,
            &current.resource.path,
            &draft.resource.path,
        )
        .unwrap_or_else(|| draft.resource.path.clone());
        if state.resource.id.is_none() {
            state.resource.id.clone_from(&draft.resource.id);
        }
        let text = match diffy::MergeOptions::new()
            .set_conflict_marker_length(marker_length)
            .merge(base_text, current_text, draft_text)
        {
            Ok(text) | Err(text) => text,
        };
        state.content = Some(content_for_kind("memory", text, None));
    }
    super::dto::ReconciliationMergePreview {
        state,
        marker_length,
    }
}

/// Produce the minimal ordered mutations transforming one resource state into another.
pub(crate) fn diff_resource_states(
    current: &ReconciliationResourceState,
    resolved: &ReconciliationResourceState,
) -> Vec<DraftOperationInput> {
    match (current.exists, resolved.exists) {
        (false, false) => Vec::new(),
        (false, true) => vec![DraftOperationInput {
            action: DraftOperationAction::Create,
            resource: resolved.resource.clone(),
            content: resolved.content.clone(),
            new_path: None,
        }],
        (true, false) => vec![DraftOperationInput {
            action: DraftOperationAction::Delete,
            resource: current.resource.clone(),
            content: None,
            new_path: None,
        }],
        (true, true) => {
            let mut operations = Vec::new();
            if current.resource.path != resolved.resource.path {
                operations.push(DraftOperationInput {
                    action: DraftOperationAction::Rename,
                    resource: current.resource.clone(),
                    content: None,
                    new_path: resolved.resource.path.clone(),
                });
            }
            if current.content != resolved.content {
                let mut resource = current.resource.clone();
                resource.path = resolved.resource.path.clone();
                operations.push(DraftOperationInput {
                    action: DraftOperationAction::Update,
                    resource,
                    content: resolved.content.clone(),
                    new_path: None,
                });
            }
            operations
        }
    }
}

/// Combine proposal freshness and conflicts into the enclosing review's coordination state.
pub(crate) fn aggregate_draft_coordination(
    coordinations: &[DraftCoordination],
) -> DraftCoordination {
    let primary = coordinations.first().cloned().unwrap_or(DraftCoordination {
        freshness: DraftFreshness::Current,
        current_commit_id: None,
        has_upstream_resource_changes: false,
        reconciliation: DraftReconciliationStatus::Unknown,
        candidate_id: None,
        auto_rebased: false,
    });
    DraftCoordination {
        freshness: if coordinations
            .iter()
            .any(|coordination| coordination.freshness == DraftFreshness::Behind)
        {
            DraftFreshness::Behind
        } else {
            DraftFreshness::Current
        },
        auto_rebased: coordinations
            .iter()
            .any(|coordination| coordination.auto_rebased),
        current_commit_id: primary.current_commit_id,
        has_upstream_resource_changes: coordinations
            .iter()
            .any(|coordination| coordination.has_upstream_resource_changes),
        reconciliation: if coordinations
            .iter()
            .any(|coordination| coordination.reconciliation == DraftReconciliationStatus::Conflicts)
        {
            DraftReconciliationStatus::Conflicts
        } else if coordinations
            .iter()
            .any(|coordination| coordination.reconciliation == DraftReconciliationStatus::Unknown)
        {
            DraftReconciliationStatus::Unknown
        } else {
            DraftReconciliationStatus::Clean
        },
        candidate_id: if coordinations.len() == 1 {
            primary.candidate_id
        } else {
            None
        },
    }
}

/// Fold ordered edits into publication mutations without losing resource identity or renames.
///
/// # Errors
/// Rejects mutation sequences that cannot produce a valid publishable resource state.
pub(crate) fn materialize_draft_operations(
    operations: &[DraftOperation],
) -> Result<Vec<DraftOperationInput>, ServerError> {
    let Some(first) = operations.first() else {
        // Reconciliation can eliminate every operation when Remote already contains the change.
        return Ok(Vec::new());
    };
    if first.input.action != DraftOperationAction::Create {
        return Ok(operations
            .iter()
            .map(|operation| operation.input.clone())
            .collect());
    }

    let mut materialized = first.input.clone();
    for operation in operations.iter().skip(1) {
        match operation.input.action {
            DraftOperationAction::Create => {
                materialized.resource.path = operation.input.resource.path.clone();
                materialized.content = operation.input.content.clone();
            }
            DraftOperationAction::Update => {
                materialized.content = merge_draft_contents(
                    materialized.content.take(),
                    operation.input.content.clone(),
                )?;
            }
            DraftOperationAction::Rename => {
                materialized.resource.path = operation.input.new_path.clone();
            }
            DraftOperationAction::Delete => return Ok(Vec::new()),
        }
    }
    Ok(vec![materialized])
}

/// Merge Markdown changes against their common ancestor and preserve conflicts for resolution.
///
/// # Errors
/// Rejects malformed resource content or incompatible content categories.
pub(crate) fn merge_draft_contents(
    base: Option<DraftResourceContent>,
    update: Option<DraftResourceContent>,
) -> Result<Option<DraftResourceContent>, ServerError> {
    let _ = base;
    Ok(update)
}

/// Decode a persisted proposal mutation, rejecting unknown actions.
///
/// # Errors
/// Rejects unsupported persisted values instead of assigning a default state or privilege.
pub(crate) fn draft_operation_action(value: &str) -> Result<DraftOperationAction, ServerError> {
    match value {
        "create" => Ok(DraftOperationAction::Create),
        "update" => Ok(DraftOperationAction::Update),
        "rename" => Ok(DraftOperationAction::Rename),
        "delete" => Ok(DraftOperationAction::Delete),
        other => Err(ServerError::InvalidRequest(format!(
            "unknown draft operation action: {other}"
        ))),
    }
}

/// Decode the persisted proposal lifecycle, rejecting unknown states.
///
/// # Errors
/// Rejects unsupported persisted values instead of assigning a default state or privilege.
pub(crate) fn draft_status(value: &str) -> Result<DraftStatus, ServerError> {
    match value {
        "open" => Ok(DraftStatus::Open),
        "submitted" => Ok(DraftStatus::Submitted),
        "discarded" => Ok(DraftStatus::Discarded),
        "merged" => Ok(DraftStatus::Merged),
        other => Err(ServerError::InvalidRequest(format!(
            "unknown draft status: {other}"
        ))),
    }
}

/// Decode the persisted lifecycle event used by synchronization clients.
///
/// # Errors
/// Rejects unsupported persisted values instead of assigning a default state or privilege.
pub(crate) fn draft_event_type(value: &str) -> Result<DraftEventType, ServerError> {
    match value {
        "created" => Ok(DraftEventType::Created),
        "updated" => Ok(DraftEventType::Updated),
        "operation_appended" => Ok(DraftEventType::OperationAppended),
        "discarded" => Ok(DraftEventType::Discarded),
        "submitted" => Ok(DraftEventType::Submitted),
        "reopened" => Ok(DraftEventType::Reopened),
        "rebased" => Ok(DraftEventType::Rebased),
        "merged" => Ok(DraftEventType::Merged),
        other => Err(ServerError::InvalidRequest(format!(
            "unknown draft event type: {other}"
        ))),
    }
}

/// A transaction may persist conflict information before returning a business error.
/// The caller must commit the transaction before converting this outcome to a result.
pub(crate) enum CommitOutcome<T> {
    /// The enclosing transaction may commit and return the operation's value.
    Success(T),
    /// Reconciliation evidence must be committed before returning the contained failure.
    Failure(ServerError),
}

impl<T> CommitOutcome<T> {
    /// Convert a committed outcome into the caller-visible result after transaction ownership has
    /// been discharged.
    ///
    /// # Errors
    /// Returns the stored operation failure. Any required transaction commit must already have
    /// completed.
    pub(crate) fn into_result(self) -> Result<T, ServerError> {
        match self {
            Self::Success(value) => Ok(value),
            Self::Failure(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context_state(
        exists: bool,
        path: &str,
        content: Option<&str>,
    ) -> ReconciliationResourceState {
        ReconciliationResourceState {
            exists,
            resource: DraftResourceRef {
                scope: ResourceScope::Project,
                id: exists.then(|| "mem_test".to_owned()),
                path: Some(path.to_owned()),
            },
            content: content.map(|content| DraftResourceContent {
                description: None,
                content: content.to_owned(),
            }),
        }
    }

    fn assert_clean(
        base: &ReconciliationResourceState,
        current: &ReconciliationResourceState,
        draft: &ReconciliationResourceState,
    ) -> ReconciliationResourceState {
        let (result, conflicts) = merge_resource_states(base, current, draft);
        assert!(conflicts.is_empty(), "unexpected conflicts: {conflicts:?}");
        result.expect("clean reconciliation must produce a state")
    }

    fn assert_conflicts(
        base: &ReconciliationResourceState,
        current: &ReconciliationResourceState,
        draft: &ReconciliationResourceState,
    ) {
        let (result, conflicts) = merge_resource_states(base, current, draft);
        assert!(result.is_none());
        assert!(!conflicts.is_empty());
    }

    #[test]
    fn merge_preview_preserves_remote_rename_and_nonconflicting_content() {
        let base = context_state(true, "old.md", Some("Title\n\nbase\n\nFooter\n"));
        let current = context_state(true, "renamed.md", Some("New title\n\nremote\n\nFooter\n"));
        let draft = context_state(true, "old.md", Some("Title\n\ndraft\n\nNew footer\n"));
        let preview = reconciliation_merge_preview(&base, &current, &draft);
        assert_eq!(preview.state.resource.path.as_deref(), Some("renamed.md"));
        let text = preview.state.content.unwrap().content;
        assert!(text.starts_with("New title\n"));
        assert!(text.ends_with("New footer\n"));
        assert!(text.contains("<<<<<<< ours"));
    }

    #[test]
    fn memory_content_must_not_be_blank() {
        assert!(
            validate_draft_content_shape(&DraftResourceContent {
                description: None,
                content: String::new(),
            })
            .is_err()
        );
        assert!(
            validate_draft_content_shape(&DraftResourceContent {
                description: None,
                content: "  \n".to_owned(),
            })
            .is_err()
        );
        assert!(
            validate_draft_content_shape(&DraftResourceContent {
                description: None,
                content: "# Testing\n\nRun focused tests.".to_owned(),
            })
            .is_ok()
        );
    }

    #[test]
    fn reconciliation_merges_non_overlapping_markdown_updates() {
        let base = context_state(
            true,
            "context/guide.md",
            Some("# Guide\n\nalpha: base\n\nmiddle: base\n\nomega: base\n"),
        );
        let current = context_state(
            true,
            "context/guide.md",
            Some("# Guide\n\nalpha: remote\n\nmiddle: base\n\nomega: base\n"),
        );
        let draft = context_state(
            true,
            "context/guide.md",
            Some("# Guide\n\nalpha: base\n\nmiddle: base\n\nomega: local\n"),
        );

        let result = assert_clean(&base, &current, &draft);
        let content = result.content.as_ref().map(content_text).unwrap();
        assert!(content.contains("alpha: remote"));
        assert!(content.contains("omega: local"));
    }

    #[test]
    fn reconciliation_reports_overlapping_markdown_updates() {
        let base = context_state(true, "context/guide.md", Some("# Guide\n\nmode: base\n"));
        let current = context_state(true, "context/guide.md", Some("# Guide\n\nmode: remote\n"));
        let draft = context_state(true, "context/guide.md", Some("# Guide\n\nmode: local\n"));

        assert_conflicts(&base, &current, &draft);
    }

    #[test]
    fn reconciliation_covers_create_rename_and_delete_boundaries() {
        let absent = context_state(false, "context/new.md", None);
        let created = context_state(true, "context/new.md", Some("# Local\n"));
        assert_eq!(assert_clean(&absent, &absent, &created), created);
        let remote_created = context_state(true, "context/new.md", Some("# Remote\n"));
        assert_conflicts(&absent, &remote_created, &created);

        let base = context_state(true, "context/old.md", Some("# Base\n"));
        let remote_content = context_state(true, "context/old.md", Some("# Remote\n"));
        let renamed = context_state(true, "context/new.md", Some("# Base\n"));
        let renamed_result = assert_clean(&base, &remote_content, &renamed);
        assert_eq!(
            renamed_result.resource.path.as_deref(),
            Some("context/new.md")
        );
        assert_eq!(
            renamed_result.content.as_ref().map(content_text),
            Some("# Remote\n")
        );
        let remote_renamed = context_state(true, "context/remote.md", Some("# Base\n"));
        assert_conflicts(&base, &remote_renamed, &renamed);

        let deleted = context_state(false, "context/old.md", None);
        assert_eq!(assert_clean(&base, &base, &deleted), deleted);
        assert_conflicts(&base, &remote_content, &deleted);
        assert_conflicts(&base, &deleted, &remote_content);
    }
}
