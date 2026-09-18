//! Internal values and invariants for memory resources.

use super::dto;
use crate::app::draft::dto::DraftResourceContent;
use crate::error::ServerError;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn validate_resource_path(path: &str) -> Result<(), ServerError> {
    if !is_normalized_relative_path(path) {
        return Err(ServerError::InvalidRequest(format!(
            "resource path is not a portable normalized relative path: {path}"
        )));
    }
    Ok(())
}

pub(crate) fn is_normalized_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.ends_with('/')
        && path.split('/').all(is_portable_path_segment)
}

pub(crate) fn is_portable_path_segment(segment: &str) -> bool {
    if segment.is_empty()
        || segment == "."
        || segment == ".."
        || segment.trim() != segment
        || segment.ends_with('.')
        || segment.chars().any(|character| {
            character.is_control()
                || matches!(character, '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*')
        })
    {
        return false;
    }
    let stem = segment
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !reserved_numbered_name(&stem, "COM")
        && !reserved_numbered_name(&stem, "LPT")
}

pub(crate) fn reserved_numbered_name(stem: &str, prefix: &str) -> bool {
    stem.strip_prefix(prefix)
        .is_some_and(|suffix| matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
}

pub(crate) fn materialization_output_path(path: &str) -> Result<String, ServerError> {
    Ok(format!("cache/memory/{path}"))
}

pub(crate) fn insert_materialization_path(
    paths: &mut BTreeMap<String, (String, String)>,
    resource_id: &str,
    output_path: &str,
    owner: &str,
) -> Result<(), ServerError> {
    let normalized = output_path.to_lowercase();
    if let Some((existing_id, existing_path)) = paths.get(&normalized) {
        return Err(ServerError::InvalidRequest(format!(
            "{owner} materializes {existing_id} at {existing_path} and {resource_id} at {output_path}, which conflict"
        )));
    }
    for (index, _) in normalized.rmatch_indices('/') {
        if let Some((existing_id, existing_path)) = paths.get(&normalized[..index]) {
            return Err(ServerError::InvalidRequest(format!(
                "{owner} materializes {existing_id} at {existing_path} and {resource_id} at {output_path}, which conflict"
            )));
        }
    }
    let descendant_prefix = format!("{normalized}/");
    if let Some((_, (existing_id, existing_path))) = paths
        .range(descendant_prefix.clone()..)
        .next()
        .filter(|(path, _)| path.starts_with(&descendant_prefix))
    {
        return Err(ServerError::InvalidRequest(format!(
            "{owner} materializes {existing_id} at {existing_path} and {resource_id} at {output_path}, which conflict"
        )));
    }
    paths.insert(normalized, (resource_id.to_owned(), output_path.to_owned()));
    Ok(())
}

pub(crate) fn content_hash(body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

pub(crate) fn object_id(kind: &str, content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(kind.as_bytes());
    hasher.update([0]);
    hasher.update(content);
    hex::encode(hasher.finalize())
}

pub(crate) fn name_from_path(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

pub(crate) fn resource_scope(value: &str) -> Result<dto::ResourceScope, ServerError> {
    match value {
        "org" => Ok(dto::ResourceScope::Org),
        "project" => Ok(dto::ResourceScope::Project),
        other => Err(ServerError::InvalidRequest(format!(
            "unknown resource scope: {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_paths_follow_the_portable_file_contract() {
        assert!(validate_resource_path("spec/API.md").is_ok());
        assert!(validate_resource_path("workflow/CODING").is_ok());
        for path in [
            "../outside.md",
            "spec//API.md",
            "spec/AUX.md",
            "spec/API.md ",
            "spec/API\\draft.md",
        ] {
            assert!(
                validate_resource_path(path).is_err(),
                "path should be rejected: {path}"
            );
        }
    }
}

pub(crate) fn etag(revision: i64) -> String {
    format!("\"rev-{revision}\"")
}

pub(crate) struct PreparedResourceContent {
    pub(crate) name: String,
    pub(crate) body: String,
}

pub(crate) fn prepare_resource_content(
    path: &str,
    content: &DraftResourceContent,
    existing: Option<&TargetResource>,
) -> Result<PreparedResourceContent, ServerError> {
    let body = &content.content;
    Ok(PreparedResourceContent {
        name: existing
            .map(|resource| resource.name.clone())
            .unwrap_or_else(|| name_from_path(path)),
        body: body.clone(),
    })
}

#[derive(Default)]
pub(crate) struct OrgResourceImpact {
    pub(crate) resource_ids: BTreeSet<String>,
    pub(crate) deleted_resource_ids: BTreeSet<String>,
}

#[derive(Debug)]
pub(crate) struct TargetResource {
    pub(crate) resource_id: String,
    pub(crate) path: String,
    pub(crate) name: String,
}

pub(crate) fn resource_status(value: &str) -> Result<dto::ResourceStatus, ServerError> {
    match value {
        "active" => Ok(dto::ResourceStatus::Active),
        "deprecated" => Ok(dto::ResourceStatus::Deprecated),
        "archived" => Ok(dto::ResourceStatus::Archived),
        other => Err(ServerError::InvalidRequest(format!(
            "unknown resource status: {other}"
        ))),
    }
}
