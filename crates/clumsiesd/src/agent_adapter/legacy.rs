//! Read-only discovery for installations written by the archived Zig adapter.
//!
//! The retired adapter stored ownership manifests outside the daemon database.
//! Those manifests were never authenticated and older releases used several
//! incompatible resource topologies.  Treating one as native ownership would
//! let an untrusted JSON file authorize overwrites in an Agent host's config.
//!
//! This module therefore has a deliberately narrow contract: it discovers
//! bounded, regular `adapter-install/v1` manifests and reports what the user
//! must review.  It never adopts ownership, renames a manifest, or writes host
//! configuration.  Daemon-owned records are reconciled separately by the
//! normal native installer.

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use super::*;
use crate::util::home_dir;

const LEGACY_SCHEMA: &str = "adapter-install/v1";
const LEGACY_MANIFEST: &str = "manifest.json";
const MAX_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;
const MAX_INSTALLS: usize = 1_024;
const MAX_INSTALL_ID_BYTES: usize = 256;
const MAX_PATH_BYTES: usize = 4_096;
const MAX_RESOURCES: usize = 1_024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyManifest {
    schema_version: String,
    install_id: String,
    #[serde(default)]
    adapter_id: String,
    #[serde(default)]
    target_agent: String,
    scope: String,
    target_root: String,
    status: String,
    active_revision: u32,
    managed_resources: Vec<LegacyManagedResource>,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyManagedResource {
    resource_id: String,
    #[serde(default = "plain_file_kind")]
    resource_kind: String,
    relative_path: String,
    absolute_path: Option<String>,
    ownership: String,
    fingerprint: String,
    managed_content: Option<String>,
    active: bool,
}

fn plain_file_kind() -> String {
    "plain_file".to_owned()
}

pub(super) async fn inspect(
    _state: &DaemonState,
    request: DaemonLegacyAgentAdapterInspectionRequest,
) -> Result<DaemonLegacyAgentAdapterInspectionResponse, DaemonError> {
    let home = home_dir()?;
    inspect_at(home, request).await
}

async fn inspect_at(
    home: PathBuf,
    _request: DaemonLegacyAgentAdapterInspectionRequest,
) -> Result<DaemonLegacyAgentAdapterInspectionResponse, DaemonError> {
    // Keep the runtime path in the request for wire compatibility with older
    // Apps. This read-only discovery never launches or installs that runtime;
    // the native installer validates it at the write boundary instead.
    let installs_root = home.join(".clumsies/adapters/installs");
    // This is advisory filesystem discovery, not part of native Adapter
    // ownership. Keep synchronous directory I/O off Tokio's worker threads
    // and do not hold the native setup lock while an old store is slow or
    // inaccessible.
    tokio::task::spawn_blocking(move || discover_at(&home, &installs_root))
        .await
        .map_err(|error| {
            DaemonError::Server(format!("Archived Adapter inspection task failed: {error}"))
        })?
}

fn discover_at(
    home: &Path,
    installs_root: &Path,
) -> Result<DaemonLegacyAgentAdapterInspectionResponse, DaemonError> {
    let mut response = DaemonLegacyAgentAdapterInspectionResponse {
        scanned: 0,
        deferred: 0,
        conflicts: Vec::new(),
    };

    let root_metadata = match fs::symlink_metadata(installs_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(response),
        Err(error) => return Err(error.into()),
    };
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(adapter_conflict(
            "The archived Adapter manifest store is not a regular directory.",
        ));
    }

    let mut entries = fs::read_dir(installs_root)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    if entries.len() > MAX_INSTALLS {
        return Err(adapter_conflict(
            "The archived Adapter manifest store exceeds the supported discovery limit.",
        ));
    }

    for entry in entries {
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        response.scanned += 1;
        let directory_install_id = entry.file_name().to_string_lossy().into_owned();
        if !bounded_identifier(&directory_install_id) {
            continue;
        }

        let manifest_path = entry.path().join(LEGACY_MANIFEST);
        let Some(raw_manifest) = read_regular_bounded(&manifest_path, MAX_MANIFEST_BYTES)? else {
            continue;
        };
        let manifest = match serde_json::from_slice::<LegacyManifest>(&raw_manifest) {
            Ok(manifest) => manifest,
            Err(_) => continue,
        };
        let Some(adapter) = parse_adapter(&manifest) else {
            continue;
        };
        let reported_scope = parse_scope(&manifest.scope).unwrap_or("unknown");

        if !manifest_is_bounded(&manifest) {
            response.conflicts.push(conflict(
                &manifest,
                adapter,
                reported_scope,
                "legacy_adapter_manifest_invalid",
                "The archived Adapter manifest exceeds safe discovery limits and was left unchanged.",
            ));
            continue;
        }
        if manifest.status != "active" {
            continue;
        }
        if manifest.schema_version != LEGACY_SCHEMA
            || manifest.install_id != directory_install_id
            || manifest.adapter_id != manifest.target_agent
            || manifest.active_revision == 0
            || !absolute_normal_path(Path::new(&manifest.target_root))
        {
            response.conflicts.push(conflict(
                &manifest,
                adapter,
                reported_scope,
                "legacy_adapter_manifest_invalid",
                "The archived Adapter manifest is not a valid active installation and was left unchanged.",
            ));
            continue;
        }

        let Some(scope) = parse_scope(&manifest.scope) else {
            response.conflicts.push(conflict(
                &manifest,
                adapter,
                "unknown",
                "legacy_adapter_generation_unsupported",
                "This active archived integration has an unsupported scope. Remove its old Clumsies MCP and hook entries before reinstalling from the App.",
            ));
            continue;
        };

        // The first adapter-install/v1 generation called repository scope
        // `repo`. It is a real shipped shape, but it predates the final
        // workspace topology that the App can safely reason about. Never
        // silently skip it: the old host entry can still launch the archived
        // Zig runtime even though Clumsies cannot adopt its ownership.
        if scope == "repo" {
            response.conflicts.push(conflict(
                &manifest,
                adapter,
                scope,
                "legacy_adapter_generation_unsupported",
                "This early repository-scoped integration uses an unsupported archived layout. Remove its old Clumsies MCP and hook entries, then reinstall a project-scoped integration from the App.",
            ));
            continue;
        }

        let target_root = PathBuf::from(&manifest.target_root);
        if !target_matches_scope(home, adapter, scope, &target_root) {
            response.conflicts.push(conflict(
                &manifest,
                adapter,
                scope,
                "legacy_adapter_target_mismatch",
                "The archived Adapter target does not match its declared scope and was left unchanged.",
            ));
            continue;
        }
        match fs::symlink_metadata(&target_root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                response.deferred += 1;
                continue;
            }
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                response.conflicts.push(conflict(
                    &manifest,
                    adapter,
                    scope,
                    "legacy_adapter_target_invalid",
                    "The archived Adapter target is not a regular directory and was left unchanged.",
                ));
                continue;
            }
            Ok(_) => {}
            Err(_) => {
                response.deferred += 1;
                continue;
            }
        }

        let message = if scope == "user" {
            "This user-wide integration was installed by the archived Zig CLI. Clumsies left it unchanged and no longer installs global integrations; remove the legacy global Clumsies MCP and hook entries, then enable the integration for each repository in the App."
        } else {
            "This workspace integration was installed by the archived Zig CLI. Clumsies left it unchanged; remove or disable its old Clumsies MCP and hook entries, then reinstall the integration from the App's Project settings."
        };
        response.conflicts.push(conflict(
            &manifest,
            adapter,
            scope,
            "legacy_adapter_manual_reinstall_required",
            message,
        ));
    }

    Ok(response)
}

fn read_regular_bounded(path: &Path, limit: u64) -> Result<Option<Vec<u8>>, DaemonError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > limit {
        return Ok(None);
    }
    let content = fs::read(path)?;
    if content.len() as u64 > limit {
        return Ok(None);
    }
    Ok(Some(content))
}

fn parse_adapter(manifest: &LegacyManifest) -> Option<ProjectAgentAdapterKind> {
    if manifest.adapter_id != manifest.target_agent {
        return None;
    }
    match manifest.adapter_id.as_str() {
        "codex" => Some(ProjectAgentAdapterKind::Codex),
        "claude-code" => Some(ProjectAgentAdapterKind::ClaudeCode),
        _ => None,
    }
}

fn parse_scope(scope: &str) -> Option<&'static str> {
    match scope {
        "workspace" => Some("workspace"),
        "user" => Some("user"),
        "repo" => Some("repo"),
        _ => None,
    }
}

fn conflict(
    manifest: &LegacyManifest,
    adapter: ProjectAgentAdapterKind,
    scope: &str,
    code: &str,
    message: &str,
) -> DaemonLegacyAgentAdapterConflict {
    DaemonLegacyAgentAdapterConflict {
        install_id: manifest.install_id.clone(),
        adapter,
        scope: scope.to_owned(),
        target_root: manifest.target_root.clone(),
        code: code.to_owned(),
        message: message.to_owned(),
    }
}

fn bounded_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_INSTALL_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn manifest_is_bounded(manifest: &LegacyManifest) -> bool {
    bounded_identifier(&manifest.install_id)
        && manifest.scope.len() <= 64
        && manifest.target_root.len() <= MAX_PATH_BYTES
        && manifest.managed_resources.len() <= MAX_RESOURCES
        && manifest.managed_resources.iter().all(|resource| {
            resource.resource_id.len() <= 512
                && resource.resource_kind.len() <= 64
                && resource.relative_path.len() <= MAX_PATH_BYTES
                && resource
                    .absolute_path
                    .as_ref()
                    .is_none_or(|path| path.len() <= MAX_PATH_BYTES)
                && resource.ownership.len() <= 64
                && resource.fingerprint.len() <= 128
                && resource
                    .managed_content
                    .as_ref()
                    .is_none_or(|content| content.len() <= MAX_MANIFEST_BYTES as usize)
                && (!resource.active || !resource.resource_id.is_empty())
        })
        && manifest.created_at >= 0
        && manifest.updated_at >= 0
}

fn absolute_normal_path(path: &Path) -> bool {
    path.is_absolute()
        && path.components().all(|component| {
            !matches!(
                component,
                Component::CurDir | Component::ParentDir | Component::Prefix(_)
            )
        })
}

fn target_matches_scope(
    home: &Path,
    adapter: ProjectAgentAdapterKind,
    scope: &str,
    target: &Path,
) -> bool {
    match (adapter, scope) {
        (ProjectAgentAdapterKind::Codex, "user") => target == home.join(".codex"),
        (ProjectAgentAdapterKind::ClaudeCode, "user") => target == home,
        (ProjectAgentAdapterKind::Codex, "workspace" | "repo") => {
            target.file_name().and_then(|name| name.to_str()) == Some(".codex")
                && target.parent().is_some_and(|parent| parent != home)
        }
        (ProjectAgentAdapterKind::ClaudeCode, "workspace" | "repo") => target != home,
        (ProjectAgentAdapterKind::Opencode, _) | (_, _) => false,
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    fn write_manifest(
        installs: &Path,
        install_id: &str,
        adapter: &str,
        scope: &str,
        target: &Path,
    ) -> Vec<u8> {
        let directory = installs.join(install_id);
        fs::create_dir_all(&directory).unwrap();
        let raw = serde_json::to_vec_pretty(&json!({
            "schema_version": LEGACY_SCHEMA,
            "install_id": install_id,
            "adapter_id": adapter,
            "target_agent": adapter,
            "scope": scope,
            "target_root": target,
            "status": "active",
            "active_revision": 1,
            "managed_resources": [],
            "created_at": 1,
            "updated_at": 1
        }))
        .unwrap();
        fs::write(directory.join(LEGACY_MANIFEST), &raw).unwrap();
        raw
    }

    #[test]
    fn missing_store_is_empty() {
        let root = TempDir::new().unwrap();
        let result = discover_at(root.path(), &root.path().join("missing")).unwrap();
        assert_eq!(result.scanned, 0);
        assert!(result.conflicts.is_empty());
    }

    #[tokio::test]
    async fn inspection_does_not_require_an_agent_runtime() {
        let root = TempDir::new().unwrap();
        let result = inspect_at(
            root.path().to_path_buf(),
            DaemonLegacyAgentAdapterInspectionRequest {
                runtime_binary_path: "/missing/Clumsies.app/Contents/Resources/clumsiesd"
                    .to_owned(),
            },
        )
        .await
        .unwrap();

        assert_eq!(result.scanned, 0);
        assert!(result.conflicts.is_empty());
    }

    #[test]
    fn reachable_legacy_install_is_reported_without_mutation() {
        let root = TempDir::new().unwrap();
        let installs = root.path().join("installs");
        let workspace = root.path().join("workspace");
        let target = workspace.join(".codex");
        fs::create_dir_all(&target).unwrap();
        let original = write_manifest(&installs, "install_1", "codex", "workspace", &target);

        let result = discover_at(root.path(), &installs).unwrap();
        assert_eq!(result.scanned, 1);
        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(
            result.conflicts[0].code,
            "legacy_adapter_manual_reinstall_required"
        );
        assert_eq!(
            fs::read(installs.join("install_1/manifest.json")).unwrap(),
            original
        );
        assert!(!installs.join("install_1/manifest.migrated.json").exists());
        assert!(!installs.join("install_1/migration.json").exists());
    }

    #[test]
    fn missing_workspace_is_deferred_without_mutation() {
        let root = TempDir::new().unwrap();
        let installs = root.path().join("installs");
        let target = root.path().join("offline-workspace/.codex");
        let original = write_manifest(&installs, "install_2", "codex", "workspace", &target);

        let result = discover_at(root.path(), &installs).unwrap();
        assert_eq!(result.deferred, 1);
        assert!(result.conflicts.is_empty());
        assert_eq!(
            fs::read(installs.join("install_2/manifest.json")).unwrap(),
            original
        );
    }

    #[test]
    fn symlink_manifest_is_never_followed() {
        let root = TempDir::new().unwrap();
        let installs = root.path().join("installs");
        let install = installs.join("install_3");
        fs::create_dir_all(&install).unwrap();
        let target = root.path().join("workspace/.codex");
        fs::create_dir_all(&target).unwrap();
        let outside = root.path().join("outside.json");
        fs::write(&outside, b"{}").unwrap();
        symlink(&outside, install.join(LEGACY_MANIFEST)).unwrap();

        let result = discover_at(root.path(), &installs).unwrap();
        assert_eq!(result.scanned, 1);
        assert!(result.conflicts.is_empty());
        assert_eq!(fs::read(&outside).unwrap(), b"{}");
    }

    #[test]
    fn candidates_are_discovered_independently() {
        let root = TempDir::new().unwrap();
        let installs = root.path().join("installs");
        let reachable = root.path().join("one/.codex");
        fs::create_dir_all(&reachable).unwrap();
        write_manifest(&installs, "one", "codex", "workspace", &reachable);
        write_manifest(
            &installs,
            "two",
            "claude-code",
            "workspace",
            &root.path().join("offline"),
        );

        let result = discover_at(root.path(), &installs).unwrap();
        assert_eq!(result.scanned, 2);
        assert_eq!(result.deferred, 1);
        assert_eq!(result.conflicts.len(), 1);
    }

    #[test]
    fn shipped_repo_scope_is_reported_as_unsupported_without_mutation() {
        let root = TempDir::new().unwrap();
        let installs = root.path().join("installs");
        let target = root.path().join("old-repository/.codex");
        fs::create_dir_all(&target).unwrap();
        let original = write_manifest(&installs, "repo-era", "codex", "repo", &target);

        let result = discover_at(root.path(), &installs).unwrap();
        assert_eq!(result.scanned, 1);
        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].scope, "repo");
        assert_eq!(
            result.conflicts[0].code,
            "legacy_adapter_generation_unsupported"
        );
        assert_eq!(
            fs::read(installs.join("repo-era/manifest.json")).unwrap(),
            original
        );
    }

    #[test]
    fn unknown_active_scope_is_reported_instead_of_silently_skipped() {
        let root = TempDir::new().unwrap();
        let installs = root.path().join("installs");
        let target = root.path().join("old-target/.codex");
        fs::create_dir_all(&target).unwrap();
        write_manifest(&installs, "unknown-era", "codex", "machine", &target);

        let result = discover_at(root.path(), &installs).unwrap();
        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].scope, "unknown");
        assert_eq!(
            result.conflicts[0].code,
            "legacy_adapter_generation_unsupported"
        );
    }
}
