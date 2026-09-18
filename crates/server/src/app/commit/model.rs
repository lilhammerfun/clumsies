//! Internal values and invariants for commit resources.

use crate::app::commit::dto::{CommitScope, TreeEntryKind, TreeEntryScope, TreeEntrySource};
use crate::app::memory::model::{
    insert_materialization_path, materialization_output_path, validate_resource_path,
};
use crate::error::ServerError;
use std::collections::BTreeMap;

#[derive(serde::Serialize)]
pub(crate) struct PendingTreeEntry {
    pub(crate) item_id: String,
    pub(crate) resource_kind: String,
    pub(crate) scope: String,
    pub(crate) project_id: Option<String>,
    pub(crate) path: Option<String>,
    pub(crate) blob_id: String,
    pub(crate) source: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) description: String,
}

pub(crate) fn validate_tree_materialization_paths(
    entries: &[PendingTreeEntry],
) -> Result<(), ServerError> {
    let mut paths = BTreeMap::new();
    for entry in entries {
        if entry.resource_kind == "project_org_selection" {
            continue;
        }
        let path = entry.path.as_deref().ok_or_else(|| {
            ServerError::InvalidRequest(format!(
                "Commit Tree entry {} is missing a path",
                entry.item_id
            ))
        })?;
        validate_resource_path(path)?;
        let output_path = materialization_output_path(path)?;
        insert_materialization_path(&mut paths, &entry.item_id, &output_path, "Commit Tree")?;
    }
    Ok(())
}

pub(crate) fn commit_scope(value: &str) -> Result<CommitScope, ServerError> {
    match value {
        "org" => Ok(CommitScope::Org),
        "project" => Ok(CommitScope::Project),
        other => Err(ServerError::InvalidRequest(format!(
            "unknown commit scope: {other}"
        ))),
    }
}

pub(crate) fn tree_entry_kind(value: &str) -> Result<TreeEntryKind, ServerError> {
    match value {
        // Legacy kinds from archived pre-unification Commits stay decodable
        // so archived history can still be read; the unified runtime only
        // writes 'memory' and the system 'project_org_selection' entry.
        "rule" | "context" | "workflow" | "memory" => Ok(TreeEntryKind::Memory),
        "project_org_selection" => Ok(TreeEntryKind::ProjectOrgSelection),
        other => Err(ServerError::InvalidRequest(format!(
            "unknown tree entry kind: {other}"
        ))),
    }
}

pub(crate) fn tree_entry_scope(value: &str) -> Result<TreeEntryScope, ServerError> {
    match value {
        "org" => Ok(TreeEntryScope::Org),
        "project" => Ok(TreeEntryScope::Project),
        "daemon" => Ok(TreeEntryScope::Daemon),
        other => Err(ServerError::InvalidRequest(format!(
            "unknown tree entry scope: {other}"
        ))),
    }
}

pub(crate) fn tree_entry_source(value: &str) -> Result<TreeEntrySource, ServerError> {
    match value {
        "org" => Ok(TreeEntrySource::Org),
        "project" => Ok(TreeEntrySource::Project),
        "selected_org" => Ok(TreeEntrySource::SelectedOrg),
        "bootstrap" => Ok(TreeEntrySource::Bootstrap),
        "config" => Ok(TreeEntrySource::Config),
        other => Err(ServerError::InvalidRequest(format!(
            "unknown tree entry source: {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_context_entry(id: &str, path: &str) -> PendingTreeEntry {
        PendingTreeEntry {
            item_id: id.to_owned(),
            resource_kind: "memory".to_owned(),
            scope: "project".to_owned(),
            project_id: Some("prj_test".to_owned()),
            path: Some(path.to_owned()),
            blob_id: format!("blob_{id}"),
            source: "project".to_owned(),
            description: String::new(),
        }
    }

    #[test]
    fn commit_trees_reject_case_and_file_directory_collisions() {
        assert!(
            validate_tree_materialization_paths(&[
                pending_context_entry("one", "spec/API.md"),
                pending_context_entry("two", "spec/api.md"),
            ])
            .is_err()
        );
        assert!(
            validate_tree_materialization_paths(&[
                pending_context_entry("one", "spec/API.md"),
                pending_context_entry("two", "spec/API.md/examples.md"),
            ])
            .is_err()
        );
        assert!(
            validate_tree_materialization_paths(&[
                pending_context_entry("one", "spec/API.md/examples.md"),
                pending_context_entry("two", "spec/API.md"),
            ])
            .is_err()
        );
        assert!(
            validate_tree_materialization_paths(&[
                pending_context_entry("one", "spec/API.md"),
                pending_context_entry("two", "spec/CLI.md"),
            ])
            .is_ok()
        );
    }
}
