//! Internal values and invariants for memory resources.

use super::dto;
use crate::app::draft::dto::DraftResourceContent;
use crate::error::ServerError;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// Require a portable normalized relative path safe to materialize on supported clients.
///
/// # Errors
/// Rejects absolute, nonnormalized, reserved, or nonportable resource paths.
pub(crate) fn validate_resource_path(path: &str) -> Result<(), ServerError> {
    if !is_normalized_relative_path(path) {
        return Err(ServerError::InvalidRequest(format!(
            "resource path is not a portable normalized relative path: {path}"
        )));
    }
    Ok(())
}

/// Check that a resource path is relative and consists only of portable nonempty segments.
pub(crate) fn is_normalized_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.ends_with('/')
        && path.split('/').all(is_portable_path_segment)
}

/// Reject traversal, reserved device names, and platform-unsafe filename characters.
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

/// Recognize numbered Windows device names that are unsafe as path segments.
pub(crate) fn reserved_numbered_name(stem: &str, prefix: &str) -> bool {
    stem.strip_prefix(prefix)
        .is_some_and(|suffix| matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
}

/// Resolve the client-visible output path while enforcing portability constraints.
///
/// # Errors
/// Rejects unsupported resource categories and paths unsafe for client materialization.
pub(crate) fn materialization_output_path(path: &str) -> Result<String, ServerError> {
    Ok(format!("cache/memory/{path}"))
}

/// Reject duplicate output destinations when assembling effective Memory.
///
/// # Errors
/// Rejects unsafe paths and collisions with a previously registered output destination.
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

/// Fingerprint resource text to detect changes without comparing entire payloads.
pub(crate) fn content_hash(body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// Build a namespaced content-addressed identifier from canonical bytes.
pub(crate) fn object_id(kind: &str, content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(kind.as_bytes());
    hasher.update([0]);
    hasher.update(content);
    hex::encode(hasher.finalize())
}

/// Derive the display name stored for a resource path.
pub(crate) fn name_from_path(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

/// Decode a persisted resource ownership scope.
///
/// # Errors
/// Rejects unsupported persisted values instead of assigning a default state or privilege.
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

/// Encode an optimistic concurrency revision as a quoted HTTP validator.
pub(crate) fn etag(revision: i64) -> String {
    format!("\"rev-{revision}\"")
}

/// Validated name and body ready for persistence after a resource mutation.
pub(crate) struct PreparedResourceContent {
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub(crate) name: String,
    /// Stored text content, including Markdown where the resource contract permits it.
    pub(crate) body: String,
}

/// Validate a resource mutation and preserve metadata required by its existing identity.
///
/// # Errors
/// Rejects mutation payloads that do not match the action's content and path requirements.
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

/// Changed and deleted organization resources whose project projections need refresh.
#[derive(Default)]
pub(crate) struct OrgResourceImpact {
    /// Resource identities selected or affected by this operation.
    pub(crate) resource_ids: BTreeSet<String>,
    /// Resources whose removal requires dependent selections to be updated.
    pub(crate) deleted_resource_ids: BTreeSet<String>,
}

/// Persisted resource identity and content loaded before applying a draft mutation.
#[derive(Debug)]
pub(crate) struct TargetResource {
    /// Stable identity of the persisted resource.
    pub(crate) resource_id: String,
    /// Resource path within its ownership scope, used to determine the materialized destination.
    pub(crate) path: String,
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub(crate) name: String,
}

/// Decode whether a persisted resource participates in current snapshots.
///
/// # Errors
/// Rejects unsupported persisted values instead of assigning a default state or privilege.
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

/// Memory content and identity required to construct a snapshot entry.
#[derive(sqlx::FromRow)]
pub(crate) struct CommitResource {
    /// Explicit immutable origin of a Project adaptation.
    pub(crate) org_source: Option<sqlx::types::Json<crate::app::memory::dto::OrgMemorySource>>,
    /// Stable identity of the persisted resource.
    pub(crate) resource_id: String,
    /// Stored text content, including Markdown where the resource contract permits it.
    pub(crate) body: String,
    /// Human-readable explanation associated with the resource.
    pub(crate) description: String,
    /// Stored content category used to validate and materialize the payload.
    pub(crate) resource_kind: String,
    /// Resource path within its ownership scope, used to determine the materialized destination.
    pub(crate) path: String,
}
