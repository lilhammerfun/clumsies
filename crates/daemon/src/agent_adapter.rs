use std::collections::{BTreeSet, HashSet};
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Connection, Row, SqlitePool};
use toml_edit::{Array, DocumentMut, Item, Table, value};
use uuid::Uuid;

use crate::{
    DaemonError, DaemonState, canonical_binding_root, canonical_server_url,
    canonical_workspace_directory, project_binding_from_row,
};

#[cfg(target_os = "macos")]
use crate::config::DAEMON_AGENT_LABEL;

mod codex_plugin;
pub(crate) mod global;
mod legacy;
pub use global::{
    DaemonAgentAdapterSetting, DaemonAgentAdapterSettings, DaemonSetAgentAdapterRequest,
};

const ISSUE_RUN_EVENT_CODEX: &str =
    include_str!("../../../assets/adapters/codex/runtime/hooks/agent-run-event.sh.tpl");
const ISSUE_RUN_EVENT_CLAUDE: &str =
    include_str!("../../../assets/adapters/claude-code/runtime/hooks/agent-run-event.sh.tpl");
const ISSUE_RUN_EVENT_ANTIGRAVITY: &str =
    include_str!("../../../assets/adapters/antigravity/runtime/hooks/agent-run-event.sh.tpl");
const OPENCODE_PLUGIN: &str = include_str!("../../../assets/adapters/opencode/runtime/plugin.ts");
const LEGACY_USER_PROMPT_SUBMIT_CODEX_SHA256: &str =
    "03bfb5ddbad36dcf53ba3f1e4e07a83cece33d4a98c29298dc0d7e776f63f815";
const LEGACY_USER_PROMPT_SUBMIT_CLAUDE_SHA256: &str =
    "6a2daa1dca1e4ae6ee5c7855bf160418fea46a3ca881c47acdd3f7e7f539aa54";
const MAX_ADAPTER_FS_OPS: usize = 128;
const MAX_ADAPTER_FS_CHANGES: usize = 32;
const MAX_ADAPTER_FS_CONTENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_ADAPTER_FS_JOURNAL_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectAgentAdapterKind {
    Codex,
    ClaudeCode,
    Opencode,
    Dsh,
    Antigravity,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectAgentAdapterDelivery {
    LegacyFiles,
    HostPlugin,
}

impl ProjectAgentAdapterKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
            Self::Opencode => "opencode",
            Self::Dsh => "dsh",
            Self::Antigravity => "antigravity",
        }
    }

    fn parse(value: &str) -> Result<Self, DaemonError> {
        match value {
            "codex" => Ok(Self::Codex),
            "claude-code" => Ok(Self::ClaudeCode),
            "opencode" => Ok(Self::Opencode),
            "dsh" => Ok(Self::Dsh),
            "antigravity" => Ok(Self::Antigravity),
            _ => Err(DaemonError::InvalidConfig(
                "persisted Coding Agent adapter kind is invalid".to_owned(),
            )),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectAgentAdapterListRequest {
    pub project_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectAgentAdapterInstallRequest {
    pub project_id: String,
    pub workspace_root: String,
    pub adapter: ProjectAgentAdapterKind,
    pub runtime_binary_path: String,
    pub host_binary_path: Option<String>,
    pub expected_revision: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectAgentAdapterRemoveRequest {
    pub workspace_root: String,
    pub adapter: ProjectAgentAdapterKind,
    pub expected_revision: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectAgentAdapter {
    pub server_url: String,
    pub project_id: String,
    pub workspace_root: String,
    pub adapter: ProjectAgentAdapterKind,
    pub delivery: ProjectAgentAdapterDelivery,
    pub revision: i64,
    pub managed_files: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectAgentAdapterListResponse {
    pub items: Vec<DaemonProjectAgentAdapter>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonLegacyAgentAdapterInspectionRequest {
    pub runtime_binary_path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonLegacyAgentAdapterConflict {
    pub install_id: String,
    pub adapter: ProjectAgentAdapterKind,
    pub scope: String,
    pub target_root: String,
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonLegacyAgentAdapterInspectionResponse {
    pub scanned: usize,
    pub deferred: usize,
    pub conflicts: Vec<DaemonLegacyAgentAdapterConflict>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectAgentAdapterRemoveResponse {
    pub workspace_root: String,
    pub adapter: ProjectAgentAdapterKind,
    pub removed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonCodexPluginRequest {
    pub runtime_binary_path: String,
    pub host_binary_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonCodexPluginStatus {
    pub host_installed: bool,
    pub marketplace_installed: bool,
    pub marketplace_conflict: bool,
    pub plugin_installed: bool,
    pub plugin_enabled: bool,
    pub installed_version: Option<String>,
    pub expected_version: String,
    pub ready: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct AdapterManifest {
    runtime_binary_hash: String,
    runtime_binary_path: String,
    delivery: ProjectAgentAdapterDelivery,
    managed_files: Vec<ManagedFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ManagedFile {
    path: String,
    kind: ManagedFileKind,
    installed_hash: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ManagedFileKind {
    CodexConfig,
    CodexHooks,
    ClaudeMcp,
    ClaudeSettings,
    OpencodeConfig,
    DshConfig,
    AntigravityHooks,
    Exclusive,
}

impl Serialize for ManagedFile {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ManagedFile", 4)?;
        state.serialize_field("path", &self.path)?;
        state.serialize_field("kind", &self.kind)?;
        state.serialize_field("installed_hash", &self.installed_hash)?;
        state.serialize_field("owned_fragments", managed_file_owned_fragments(self.kind))?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for ManagedFile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct StoredManagedFile {
            path: String,
            kind: ManagedFileKind,
            installed_hash: String,
            #[serde(default)]
            owned_fragments: Option<Vec<String>>,
        }

        let stored = StoredManagedFile::deserialize(deserializer)?;
        if let Some(declared) = &stored.owned_fragments {
            let expected = managed_file_owned_fragments(stored.kind);
            let legacy = legacy_managed_file_owned_fragments(stored.kind);
            if !owned_fragments_match(declared, expected)
                && !legacy.is_some_and(|legacy| owned_fragments_match(declared, legacy))
            {
                return Err(serde::de::Error::custom(
                    "managed adapter manifest declares unexpected owned fragments",
                ));
            }
        }
        Ok(Self {
            path: stored.path,
            kind: stored.kind,
            installed_hash: stored.installed_hash,
        })
    }
}

fn managed_file_owned_fragments(kind: ManagedFileKind) -> &'static [&'static str] {
    match kind {
        ManagedFileKind::CodexConfig => {
            &["mcp_servers.clumsies.command", "mcp_servers.clumsies.args"]
        }
        ManagedFileKind::CodexHooks | ManagedFileKind::ClaudeSettings => &[
            "hooks.UserPromptSubmit[clumsies-agent-run-event]",
            "hooks.StopFailure[clumsies-agent-run-event]",
        ],
        ManagedFileKind::ClaudeMcp => &["mcpServers.clumsies"],
        ManagedFileKind::OpencodeConfig => {
            &["mcp.clumsies", "plugin[./.opencode/plugins/clumsies.ts]"]
        }
        ManagedFileKind::DshConfig => &["$file"],
        ManagedFileKind::AntigravityHooks => &["clumsies-agent-run-event.PreInvocation"],
        ManagedFileKind::Exclusive => &["$file"],
    }
}

fn legacy_managed_file_owned_fragments(kind: ManagedFileKind) -> Option<&'static [&'static str]> {
    match kind {
        ManagedFileKind::CodexHooks | ManagedFileKind::ClaudeSettings => Some(&[
            "hooks.UserPromptSubmit[clumsies-agent-run-event]",
            "hooks.Stop[clumsies-agent-run-event]",
            "hooks.StopFailure[clumsies-agent-run-event]",
        ]),
        ManagedFileKind::AntigravityHooks => Some(&[
            "clumsies-agent-run-event.PreInvocation",
            "clumsies-agent-run-event.Stop",
        ]),
        _ => None,
    }
}

fn owned_fragments_match(declared: &[String], expected: &[&str]) -> bool {
    declared
        .iter()
        .map(String::as_str)
        .eq(expected.iter().copied())
}

#[derive(Clone, Debug)]
struct PendingChange {
    path: PathBuf,
    expected: FileSnapshot,
    desired: Option<Vec<u8>>,
    kind: ManagedFileKind,
    mode: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileSnapshot {
    content: Option<Vec<u8>>,
    mode: Option<u32>,
    #[cfg(unix)]
    identity: Option<FileIdentity>,
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[cfg(test)]
#[derive(Debug)]
struct FileBackup {
    path: PathBuf,
    before: FileSnapshot,
    after: FileSnapshot,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct JournalChange {
    relative_path: String,
    kind: ManagedFileKind,
    stage_name: Option<String>,
    before_content: Option<Vec<u8>>,
    before_mode: Option<u32>,
    after_content: Option<Vec<u8>>,
    after_mode: Option<u32>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AdapterFsAction {
    Install,
    Remove,
}

impl AdapterFsAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Remove => "remove",
        }
    }

    fn parse(value: &str) -> Result<Self, DaemonError> {
        match value {
            "install" => Ok(Self::Install),
            "remove" => Ok(Self::Remove),
            _ => Err(DaemonError::InvalidConfig(
                "persisted adapter filesystem action is invalid".to_owned(),
            )),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PreparedAdapterFsOp {
    global: bool,
    operation_id: String,
    server_url: String,
    workspace_root: PathBuf,
    project_id: String,
    adapter: ProjectAgentAdapterKind,
    action: AdapterFsAction,
    expected_revision: Option<i64>,
    next_revision: Option<i64>,
    manifest_json: Option<String>,
    changes: Vec<JournalChange>,
}

fn journal_changes_from_plan(
    workspace_root: &Path,
    changes: &[PendingChange],
) -> Result<Vec<JournalChange>, DaemonError> {
    changes
        .iter()
        .map(|change| {
            validate_manifest_managed_path(workspace_root, &change.path, change.kind)?;
            let relative = change
                .path
                .strip_prefix(workspace_root)
                .map_err(|_| adapter_conflict("A journaled adapter path escaped its workspace."))?;
            let relative = relative
                .to_str()
                .ok_or_else(|| adapter_conflict("A journaled Adapter path is not valid UTF-8."))?;
            let after_mode = desired_file_mode(change);
            Ok(JournalChange {
                relative_path: relative.to_owned(),
                kind: change.kind,
                stage_name: change
                    .desired
                    .as_ref()
                    .map(|_| format!(".clumsies-adapter-stage-{}.tmp", Uuid::new_v4().simple())),
                before_content: change.expected.content.clone(),
                before_mode: change.expected.mode,
                after_content: change.desired.clone(),
                after_mode,
            })
        })
        .collect()
}

fn journal_change_path(
    workspace_root: &Path,
    adapter: ProjectAgentAdapterKind,
    change: &JournalChange,
    global: bool,
) -> Result<PathBuf, DaemonError> {
    let relative = Path::new(&change.relative_path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(adapter_conflict(
            "An adapter filesystem journal contains an invalid relative path.",
        ));
    }
    let path = workspace_root.join(relative);
    validate_manifest_managed_path(workspace_root, &path, change.kind)?;
    if global {
        global::validate_path(adapter, relative, change.kind)?;
    } else {
        validate_adapter_journal_path(adapter, relative, change.kind)?;
    }
    Ok(path)
}

fn validate_adapter_journal_path(
    adapter: ProjectAgentAdapterKind,
    relative: &Path,
    kind: ManagedFileKind,
) -> Result<(), DaemonError> {
    // The retired thin-skill paths stay accepted so that pending journal ops
    // written before the thin-skills retirement (ISSUE-064) remain
    // recoverable. New plans never produce them.
    let allowed = match adapter {
        ProjectAgentAdapterKind::Codex => matches!(
            (relative.to_str(), kind),
            (Some(".codex/config.toml"), ManagedFileKind::CodexConfig)
                | (Some(".codex/hooks.json"), ManagedFileKind::CodexHooks)
                | (
                    Some(
                        ".codex/hooks/resolve-binary.sh"
                            | ".codex/hooks/agent-run-event.sh"
                            | ".codex/hooks/user-prompt-submit.sh"
                            | ".agents/skills/activate/SKILL.md"
                            | ".agents/skills/ntmd/SKILL.md"
                    ),
                    ManagedFileKind::Exclusive
                )
        ),
        ProjectAgentAdapterKind::ClaudeCode => matches!(
            (relative.to_str(), kind),
            (Some(".mcp.json"), ManagedFileKind::ClaudeMcp)
                | (
                    Some(".claude/settings.json"),
                    ManagedFileKind::ClaudeSettings
                )
                | (
                    Some(
                        ".claude/hooks/resolve-binary.sh"
                            | ".claude/hooks/agent-run-event.sh"
                            | ".claude/hooks/user-prompt-submit.sh"
                            | ".claude/skills/activate/SKILL.md"
                            | ".claude/skills/ntmd/SKILL.md"
                    ),
                    ManagedFileKind::Exclusive
                )
        ),
        ProjectAgentAdapterKind::Opencode => matches!(
            (relative.to_str(), kind),
            (Some("opencode.json"), ManagedFileKind::OpencodeConfig)
                | (
                    Some(".opencode/plugins/clumsies.ts"),
                    ManagedFileKind::Exclusive
                )
        ),
        ProjectAgentAdapterKind::Dsh => matches!(
            (relative.to_str(), kind),
            (Some(".dsh/clumsies.json"), ManagedFileKind::DshConfig)
        ),
        ProjectAgentAdapterKind::Antigravity => matches!(
            (relative.to_str(), kind),
            (Some(".mcp.json"), ManagedFileKind::ClaudeMcp)
                | (
                    Some(".agents/hooks.json"),
                    ManagedFileKind::AntigravityHooks
                )
                | (
                    Some(".agents/hooks/resolve-binary.sh" | ".agents/hooks/agent-run-event.sh"),
                    ManagedFileKind::Exclusive
                )
        ),
    };
    if !allowed {
        return Err(adapter_conflict(
            "An adapter filesystem journal path is outside its adapter's fixed namespace.",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct ManagedPathGuard {
    path: PathBuf,
    anchor: PathBuf,
    directories: Vec<DirectoryIdentity>,
}

#[derive(Clone, Debug)]
struct DirectoryIdentity {
    path: PathBuf,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    canonical: PathBuf,
}

pub(crate) async fn migrate(pool: &SqlitePool) -> Result<(), DaemonError> {
    global::migrate(pool).await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS project_agent_adapters (
            server_url TEXT NOT NULL,
            workspace_root TEXT NOT NULL,
            project_id TEXT NOT NULL,
            adapter TEXT NOT NULL CHECK (adapter IN ('codex', 'claude-code', 'opencode', 'dsh', 'antigravity')),
            revision BIGINT NOT NULL CHECK (revision > 0),
            manifest_json TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            PRIMARY KEY (server_url, workspace_root, adapter),
            FOREIGN KEY (server_url, workspace_root)
                REFERENCES project_bindings(server_url, workspace_root)
                ON DELETE CASCADE
        )",
    )
    .execute(pool)
    .await?;
    // The retired Zig client used helper-oriented manifest keys. Rewrite the
    // daemon-owned compact JSON once so the active type has only runtime
    // terminology; no legacy field alias remains in the production decoder.
    sqlx::query(
        "UPDATE project_agent_adapters
         SET manifest_json = replace(
             replace(manifest_json,
                 '\"helper_binary_hash\":', '\"runtime_binary_hash\":'),
                 '\"helper_binary_path\":', '\"runtime_binary_path\":')
         WHERE manifest_json LIKE '%\"helper_binary_%'",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE project_agent_adapters
         SET manifest_json = json_set(manifest_json, '$.delivery', 'legacy_files')
         WHERE json_type(manifest_json, '$.delivery') IS NULL",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_project_agent_adapters_project
         ON project_agent_adapters (server_url, project_id)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS adapter_fs_ops (
            operation_id TEXT PRIMARY KEY,
            server_url TEXT NOT NULL,
            workspace_root TEXT NOT NULL,
            project_id TEXT NOT NULL,
            adapter TEXT NOT NULL CHECK (adapter IN ('codex', 'claude-code', 'opencode', 'dsh', 'antigravity')),
            action TEXT NOT NULL CHECK (action IN ('install', 'remove')),
            expected_revision BIGINT,
            next_revision BIGINT,
            manifest_json TEXT,
            changes_json TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            UNIQUE (server_url, workspace_root, adapter)
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE adapter_fs_ops
         SET manifest_json = json_set(manifest_json, '$.delivery', 'legacy_files')
         WHERE manifest_json IS NOT NULL
           AND json_type(manifest_json, '$.delivery') IS NULL",
    )
    .execute(pool)
    .await?;
    Ok(())
}

fn prepared_adapter_fs_op(
    mut operation: PreparedAdapterFsOp,
    changes: &[PendingChange],
) -> Result<PreparedAdapterFsOp, DaemonError> {
    operation.changes = journal_changes_from_plan(&operation.workspace_root, changes)?;
    validate_prepared_adapter_fs_op(&operation)?;
    Ok(operation)
}

fn validate_prepared_adapter_fs_op(operation: &PreparedAdapterFsOp) -> Result<(), DaemonError> {
    Uuid::parse_str(&operation.operation_id)
        .map_err(|_| DaemonError::InvalidConfig("adapter journal id is invalid".to_owned()))?;
    if operation.global {
        if !operation.server_url.is_empty() || !operation.project_id.is_empty() {
            return Err(adapter_conflict(
                "A global adapter journal cannot bind a Project or Server.",
            ));
        }
    } else {
        let server_url = canonical_server_url(&operation.server_url)?;
        if server_url != operation.server_url {
            return Err(DaemonError::InvalidConfig(
                "adapter journal server URL is not canonical".to_owned(),
            ));
        }
        if operation.project_id.trim().is_empty() {
            return Err(DaemonError::InvalidConfig(
                "adapter journal project id is empty".to_owned(),
            ));
        }
    }
    let workspace =
        canonical_workspace_directory(operation.workspace_root.to_str().ok_or_else(|| {
            DaemonError::InvalidConfig("adapter journal workspace is not UTF-8".to_owned())
        })?)?;
    if workspace != operation.workspace_root {
        return Err(DaemonError::InvalidConfig(
            "adapter journal workspace is not canonical".to_owned(),
        ));
    }
    if operation.changes.len() > MAX_ADAPTER_FS_CHANGES {
        return Err(DaemonError::InvalidConfig(
            "adapter journal has an invalid file count".to_owned(),
        ));
    }

    match operation.action {
        AdapterFsAction::Install => {
            let next = operation.next_revision.ok_or_else(|| {
                DaemonError::InvalidConfig(
                    "adapter install journal has no target revision".to_owned(),
                )
            })?;
            let expected_next = operation
                .expected_revision
                .map_or(1, |revision| revision + 1);
            if next != expected_next || next <= 0 || operation.manifest_json.is_none() {
                return Err(DaemonError::InvalidConfig(
                    "adapter install journal revisions are invalid".to_owned(),
                ));
            }
        }
        AdapterFsAction::Remove => {
            if operation.expected_revision.is_none()
                || operation.next_revision.is_some()
                || operation.manifest_json.is_some()
            {
                return Err(DaemonError::InvalidConfig(
                    "adapter removal journal state is invalid".to_owned(),
                ));
            }
        }
    }

    let mut paths = HashSet::with_capacity(operation.changes.len());
    let mut content_bytes = 0usize;
    for change in &operation.changes {
        let relative = Path::new(&change.relative_path);
        if relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
            || !paths.insert(change.relative_path.clone())
        {
            return Err(DaemonError::InvalidConfig(
                "adapter journal contains an invalid or duplicate path".to_owned(),
            ));
        }
        journal_change_path(&workspace, operation.adapter, change, operation.global)?;
        validate_journal_stage_name(change)?;
        validate_journal_file_state(change.before_content.as_deref(), change.before_mode)?;
        validate_journal_file_state(change.after_content.as_deref(), change.after_mode)?;
        if let Some(content) = &change.before_content {
            content_bytes = content_bytes.saturating_add(content.len());
        }
        if let Some(content) = &change.after_content {
            content_bytes = content_bytes.saturating_add(content.len());
        }
        if content_bytes > MAX_ADAPTER_FS_JOURNAL_BYTES {
            return Err(DaemonError::InvalidConfig(
                "adapter journal file snapshots are too large".to_owned(),
            ));
        }
        let expected_after_mode = change.after_content.as_ref().map(|_| match change.kind {
            ManagedFileKind::Exclusive | ManagedFileKind::DshConfig => {
                expected_exclusive_mode(relative)
            }
            _ => change.before_mode.unwrap_or(0o644),
        });
        if change.after_mode != expected_after_mode {
            return Err(DaemonError::InvalidConfig(
                "adapter journal contains an invalid target file mode".to_owned(),
            ));
        }
    }

    if let Some(raw_manifest) = &operation.manifest_json {
        if raw_manifest.len() > MAX_ADAPTER_FS_JOURNAL_BYTES {
            return Err(DaemonError::InvalidConfig(
                "adapter journal manifest is too large".to_owned(),
            ));
        }
        let manifest: AdapterManifest = serde_json::from_str(raw_manifest)?;
        validate_absolute_normal_path(Path::new(&manifest.runtime_binary_path))?;
        if manifest.runtime_binary_hash.len() != 64
            || !manifest
                .runtime_binary_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(DaemonError::InvalidConfig(
                "adapter journal runtime identity is invalid".to_owned(),
            ));
        }
        match manifest.delivery {
            ProjectAgentAdapterDelivery::LegacyFiles => {
                if operation.changes.is_empty() {
                    return Err(DaemonError::InvalidConfig(
                        "a legacy adapter install journal cannot have an empty file plan"
                            .to_owned(),
                    ));
                }
                let desired = operation
                    .changes
                    .iter()
                    .filter_map(|change| {
                        change
                            .after_content
                            .as_deref()
                            .map(|content| (&change.relative_path, change.kind, sha256(content)))
                    })
                    .collect::<Vec<_>>();
                if manifest.managed_files.len() != desired.len() {
                    return Err(DaemonError::InvalidConfig(
                        "adapter journal manifest does not match its file plan".to_owned(),
                    ));
                }
                for managed in &manifest.managed_files {
                    let path = Path::new(&managed.path);
                    validate_manifest_managed_path(&workspace, path, managed.kind)?;
                    let relative = path.strip_prefix(&workspace).map_err(|_| {
                        DaemonError::InvalidConfig(
                            "adapter journal manifest path escaped its workspace".to_owned(),
                        )
                    })?;
                    let relative = relative.to_str().ok_or_else(|| {
                        DaemonError::InvalidConfig(
                            "adapter journal manifest path is not UTF-8".to_owned(),
                        )
                    })?;
                    let matches = desired.iter().filter(|(candidate, kind, hash)| {
                        candidate.as_str() == relative
                            && *kind == managed.kind
                            && hash == &managed.installed_hash
                    });
                    if matches.count() != 1 {
                        return Err(DaemonError::InvalidConfig(
                            "adapter journal manifest content does not match its file plan"
                                .to_owned(),
                        ));
                    }
                }
            }
            ProjectAgentAdapterDelivery::HostPlugin => {
                if operation.adapter != ProjectAgentAdapterKind::Codex
                    || !manifest.managed_files.is_empty()
                    || operation.changes.iter().any(|change| match change.kind {
                        ManagedFileKind::Exclusive => change.after_content.is_some(),
                        ManagedFileKind::CodexConfig | ManagedFileKind::CodexHooks => {
                            change.before_content.is_none() && change.after_content.is_some()
                        }
                        _ => true,
                    })
                {
                    return Err(DaemonError::InvalidConfig(
                        "a host-plugin adapter journal is not a Codex legacy cleanup plan"
                            .to_owned(),
                    ));
                }
            }
        }
    } else if operation.action == AdapterFsAction::Install {
        return Err(DaemonError::InvalidConfig(
            "adapter install journal has no manifest".to_owned(),
        ));
    }
    Ok(())
}

fn validate_journal_stage_name(change: &JournalChange) -> Result<(), DaemonError> {
    if change.after_content.is_none() {
        if change.stage_name.is_some() {
            return Err(DaemonError::InvalidConfig(
                "a journaled file deletion has an unexpected stage name".to_owned(),
            ));
        }
        return Ok(());
    }
    let name = change.stage_name.as_deref().ok_or_else(|| {
        DaemonError::InvalidConfig("a journaled file update has no stage name".to_owned())
    })?;
    let uuid = name
        .strip_prefix(".clumsies-adapter-stage-")
        .and_then(|name| name.strip_suffix(".tmp"))
        .ok_or_else(|| {
            DaemonError::InvalidConfig("adapter journal stage name is invalid".to_owned())
        })?;
    if uuid.len() != 32 || !uuid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DaemonError::InvalidConfig(
            "adapter journal stage identity is invalid".to_owned(),
        ));
    }
    Ok(())
}

fn validate_journal_file_state(
    content: Option<&[u8]>,
    mode: Option<u32>,
) -> Result<(), DaemonError> {
    if content.is_none() != mode.is_none()
        || content.is_some_and(|content| content.len() > MAX_ADAPTER_FS_CONTENT_BYTES)
        || mode.is_some_and(|mode| mode > 0o7777)
    {
        return Err(DaemonError::InvalidConfig(
            "adapter journal contains an invalid file snapshot".to_owned(),
        ));
    }
    Ok(())
}

fn expected_exclusive_mode(relative: &Path) -> u32 {
    if relative.extension() == Some(OsStr::new("sh")) {
        0o755
    } else {
        0o644
    }
}

async fn persist_prepared_adapter_fs_op(
    pool: &SqlitePool,
    operation: &PreparedAdapterFsOp,
) -> Result<(), DaemonError> {
    validate_prepared_adapter_fs_op(operation)?;
    let changes_json = serde_json::to_string(&operation.changes)?;
    if changes_json.len() > MAX_ADAPTER_FS_JOURNAL_BYTES {
        return Err(DaemonError::InvalidRequest(
            "Coding Agent integration update is too large to journal.".to_owned(),
        ));
    }

    // The prepared row is the durability barrier before any host file can
    // change. The rest of the central database uses WAL/NORMAL for throughput,
    // so temporarily strengthen only this pool connection to FULL.
    let mut connection = pool.acquire().await?;
    sqlx::query("PRAGMA synchronous = FULL")
        .execute(&mut *connection)
        .await?;
    let persist_result = async {
        let workspace = operation.workspace_root.display().to_string();
        let mut tx = (*connection).begin_with("BEGIN IMMEDIATE").await?;
        let binding_project: Option<String> = sqlx::query_scalar(
            "SELECT project_id
             FROM project_bindings
             WHERE server_url = $1 AND workspace_root = $2",
        )
        .bind(&operation.server_url)
        .bind(&workspace)
        .fetch_optional(&mut *tx)
        .await?;
        if binding_project.as_deref() != Some(operation.project_id.as_str()) {
            return Err(state_error(
                "project_binding_changed",
                "The Project binding changed while the Coding Agent integration was being prepared.",
            ));
        }

        let current_revision: Option<i64> = sqlx::query_scalar(
            "SELECT revision
             FROM project_agent_adapters
             WHERE server_url = $1 AND workspace_root = $2 AND adapter = $3",
        )
        .bind(&operation.server_url)
        .bind(&workspace)
        .bind(operation.adapter.as_str())
        .fetch_optional(&mut *tx)
        .await?;
        if current_revision != operation.expected_revision {
            return Err(state_error(
                "project_agent_adapter_changed",
                "The Coding Agent integration changed while its filesystem transaction was being prepared.",
            ));
        }

        let inserted = sqlx::query(
            "INSERT INTO adapter_fs_ops (
                 operation_id, server_url, workspace_root, project_id, adapter,
                 action, expected_revision, next_revision, manifest_json, changes_json
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (server_url, workspace_root, adapter) DO NOTHING",
        )
        .bind(&operation.operation_id)
        .bind(&operation.server_url)
        .bind(&workspace)
        .bind(&operation.project_id)
        .bind(operation.adapter.as_str())
        .bind(operation.action.as_str())
        .bind(operation.expected_revision)
        .bind(operation.next_revision)
        .bind(&operation.manifest_json)
        .bind(changes_json)
        .execute(&mut *tx)
        .await?;
        if inserted.rows_affected() != 1 {
            return Err(state_error(
                "project_agent_adapter_recovery_required",
                "A previous Coding Agent integration filesystem transaction still needs recovery.",
            ));
        }
        tx.commit().await?;
        Ok(())
    }
    .await;
    let restore_result = sqlx::query("PRAGMA synchronous = NORMAL")
        .execute(&mut *connection)
        .await;
    match (persist_result, restore_result) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error.into()),
        (Ok(()), Ok(_)) => Ok(()),
    }
}

fn prepared_adapter_fs_op_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<PreparedAdapterFsOp, DaemonError> {
    let changes_json: String = row.try_get("changes_json")?;
    if changes_json.len() > MAX_ADAPTER_FS_JOURNAL_BYTES {
        return Err(DaemonError::InvalidConfig(
            "persisted adapter filesystem journal is too large".to_owned(),
        ));
    }
    let operation = PreparedAdapterFsOp {
        global: false,
        operation_id: row.try_get("operation_id")?,
        server_url: row.try_get("server_url")?,
        workspace_root: PathBuf::from(row.try_get::<String, _>("workspace_root")?),
        project_id: row.try_get("project_id")?,
        adapter: ProjectAgentAdapterKind::parse(&row.try_get::<String, _>("adapter")?)?,
        action: AdapterFsAction::parse(&row.try_get::<String, _>("action")?)?,
        expected_revision: row.try_get("expected_revision")?,
        next_revision: row.try_get("next_revision")?,
        manifest_json: row.try_get("manifest_json")?,
        changes: serde_json::from_str(&changes_json)?,
    };
    validate_prepared_adapter_fs_op(&operation)?;
    Ok(operation)
}

async fn pending_adapter_fs_ops(
    pool: &SqlitePool,
) -> Result<Vec<PreparedAdapterFsOp>, DaemonError> {
    let rows = sqlx::query(
        "SELECT operation_id, server_url, workspace_root, project_id, adapter,
                action, expected_revision, next_revision, manifest_json, changes_json
         FROM adapter_fs_ops
         ORDER BY created_at, operation_id
         LIMIT $1",
    )
    .bind((MAX_ADAPTER_FS_OPS + 1) as i64)
    .fetch_all(pool)
    .await?;
    if rows.len() > MAX_ADAPTER_FS_OPS {
        return Err(DaemonError::InvalidConfig(
            "too many pending adapter filesystem transactions".to_owned(),
        ));
    }
    rows.iter().map(prepared_adapter_fs_op_from_row).collect()
}

fn file_snapshot_from_journal(content: &Option<Vec<u8>>, mode: Option<u32>) -> FileSnapshot {
    FileSnapshot {
        content: content.clone(),
        mode,
        #[cfg(unix)]
        identity: None,
    }
}

fn pending_change_from_journal(
    operation: &PreparedAdapterFsOp,
    change: &JournalChange,
    expected: FileSnapshot,
) -> PendingChange {
    PendingChange {
        path: operation.workspace_root.join(&change.relative_path),
        expected,
        desired: change.after_content.clone(),
        kind: change.kind,
        mode: change.after_mode.unwrap_or(0o644),
    }
}

fn apply_prepared_adapter_fs_op(
    operation: &PreparedAdapterFsOp,
) -> Result<Vec<PendingChange>, DaemonError> {
    validate_prepared_adapter_fs_op(operation)?;
    let mut applied = Vec::with_capacity(operation.changes.len());
    for (index, change) in operation.changes.iter().enumerate() {
        apply_journal_change_cas(operation, index, change)?;
        let after = file_snapshot_from_journal(&change.after_content, change.after_mode);
        applied.push(pending_change_from_journal(operation, change, after));
    }
    for (index, change) in operation.changes.iter().enumerate() {
        cleanup_journal_change_cas(operation, index, change)?;
    }
    Ok(applied)
}

#[cfg(unix)]
struct ManagedLeafDirectory {
    directory: fs::File,
    target: std::ffi::CString,
    guard: ManagedPathGuard,
}

#[cfg(unix)]
impl ManagedLeafDirectory {
    fn open(path: &Path) -> Result<Self, DaemonError> {
        use std::os::fd::FromRawFd;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

        let initial_guard = ManagedPathGuard::capture_inferred(path)?;
        let relative = path.strip_prefix(&initial_guard.anchor).map_err(|_| {
            adapter_conflict("A managed Adapter path escaped its validated workspace.")
        })?;
        let parent_relative = relative.parent().ok_or_else(|| {
            DaemonError::InvalidRequest(format!("{} has no parent directory", path.display()))
        })?;

        // Traverse and create the namespace exclusively through pinned
        // directory descriptors. A concurrent rename/symlink replacement can
        // therefore fail the final identity check, but can never redirect a
        // mkdir or leaf write outside the validated workspace.
        let anchor_metadata = fs::symlink_metadata(&initial_guard.anchor)?;
        validate_directory_metadata(&initial_guard.anchor, &anchor_metadata)?;
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let mut directory = options.open(&initial_guard.anchor)?;
        let observed_anchor = directory.metadata()?;
        if observed_anchor.dev() != anchor_metadata.dev()
            || observed_anchor.ino() != anchor_metadata.ino()
        {
            return Err(adapter_conflict(
                "The managed Adapter workspace changed while its namespace was opened.",
            ));
        }

        for component in parent_relative.components() {
            let Component::Normal(component) = component else {
                return Err(adapter_conflict(
                    "A managed Adapter directory path is not normalized.",
                ));
            };
            let name = std::ffi::CString::new(component.as_bytes()).map_err(|_| {
                DaemonError::InvalidRequest(
                    "Managed Adapter directory name contains NUL.".to_owned(),
                )
            })?;
            let open_child = |parent: &fs::File| -> std::io::Result<fs::File> {
                use std::os::fd::AsRawFd;
                let descriptor = unsafe {
                    libc::openat(
                        parent.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                };
                if descriptor < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(unsafe { fs::File::from_raw_fd(descriptor) })
                }
            };
            let next = match open_child(&directory) {
                Ok(next) => next,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    use std::os::fd::AsRawFd;
                    let created =
                        unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o755) };
                    if created != 0 {
                        let error = std::io::Error::last_os_error();
                        if error.kind() != std::io::ErrorKind::AlreadyExists {
                            return Err(error.into());
                        }
                    } else {
                        directory.sync_all()?;
                    }
                    open_child(&directory)?
                }
                Err(error) => return Err(error.into()),
            };
            if !next.metadata()?.is_dir() {
                return Err(adapter_conflict(
                    "A managed Adapter namespace component is not a directory.",
                ));
            }
            directory = next;
        }

        let guard = ManagedPathGuard::capture_inferred(path)?;
        let parent = path.parent().ok_or_else(|| {
            DaemonError::InvalidRequest(format!("{} has no parent directory", path.display()))
        })?;
        let expected = fs::symlink_metadata(parent)?;
        validate_directory_metadata(parent, &expected)?;
        let observed = directory.metadata()?;
        if observed.dev() != expected.dev() || observed.ino() != expected.ino() {
            return Err(adapter_conflict(
                "A managed Adapter directory changed while its leaf transaction was opened.",
            ));
        }
        guard.revalidate()?;
        Self::from_directory(path, directory, guard)
    }

    fn open_existing(path: &Path) -> Result<Option<Self>, DaemonError> {
        let guard = ManagedPathGuard::capture_inferred(path)?;
        let parent = path.parent().ok_or_else(|| {
            DaemonError::InvalidRequest(format!("{} has no parent directory", path.display()))
        })?;
        match fs::symlink_metadata(parent) {
            Ok(metadata) => validate_directory_metadata(parent, &metadata)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                guard.revalidate()?;
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        }
        Ok(Some(Self::open_with_guard(path, guard)?))
    }

    fn open_with_guard(path: &Path, guard: ManagedPathGuard) -> Result<Self, DaemonError> {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

        let parent = path.parent().ok_or_else(|| {
            DaemonError::InvalidRequest(format!("{} has no parent directory", path.display()))
        })?;
        let expected = fs::symlink_metadata(parent)?;
        validate_directory_metadata(parent, &expected)?;
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let directory = options.open(parent)?;
        let observed = directory.metadata()?;
        if observed.dev() != expected.dev() || observed.ino() != expected.ino() {
            return Err(adapter_conflict(
                "A managed Adapter directory changed while its leaf transaction was opened.",
            ));
        }
        guard.revalidate()?;
        Self::from_directory(path, directory, guard)
    }

    fn from_directory(
        path: &Path,
        directory: fs::File,
        guard: ManagedPathGuard,
    ) -> Result<Self, DaemonError> {
        use std::os::unix::ffi::OsStrExt;

        let target = path.file_name().ok_or_else(|| {
            DaemonError::InvalidRequest(format!("{} has no file name", path.display()))
        })?;
        let target = std::ffi::CString::new(target.as_bytes()).map_err(|_| {
            DaemonError::InvalidRequest("Managed Adapter file name contains NUL.".to_owned())
        })?;
        Ok(Self {
            directory,
            target,
            guard,
        })
    }

    fn descriptor(&self) -> std::os::fd::RawFd {
        use std::os::fd::AsRawFd;
        self.directory.as_raw_fd()
    }

    fn sync(&self) -> Result<(), DaemonError> {
        self.directory.sync_all()?;
        Ok(())
    }

    fn revalidate(&self) -> Result<(), DaemonError> {
        use std::os::unix::fs::MetadataExt;

        self.guard.revalidate()?;
        let parent = self.guard.path.parent().ok_or_else(|| {
            DaemonError::InvalidRequest(format!(
                "{} has no parent directory",
                self.guard.path.display()
            ))
        })?;
        let reachable = fs::symlink_metadata(parent)?;
        validate_directory_metadata(parent, &reachable)?;
        let pinned = self.directory.metadata()?;
        if reachable.dev() != pinned.dev() || reachable.ino() != pinned.ino() {
            return Err(adapter_conflict(
                "A managed Adapter directory was replaced during its leaf transaction.",
            ));
        }
        Ok(())
    }
}

fn journal_transient_names(
    operation: &PreparedAdapterFsOp,
    index: usize,
) -> Result<(std::ffi::CString, std::ffi::CString), DaemonError> {
    let suffix = operation.operation_id.replace('-', "");
    let old = std::ffi::CString::new(format!(".clumsies-adapter-{suffix}-{index}.old"))
        .map_err(|_| DaemonError::InvalidConfig("adapter journal name is invalid".to_owned()))?;
    let new = std::ffi::CString::new(format!(".clumsies-adapter-{suffix}-{index}.new"))
        .map_err(|_| DaemonError::InvalidConfig("adapter journal name is invalid".to_owned()))?;
    Ok((old, new))
}

#[cfg(unix)]
fn file_snapshot_at(
    directory: &ManagedLeafDirectory,
    name: &std::ffi::CStr,
) -> Result<FileSnapshot, DaemonError> {
    use std::os::fd::FromRawFd;
    use std::os::unix::fs::MetadataExt;

    let descriptor = unsafe {
        libc::openat(
            directory.descriptor(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(FileSnapshot {
                content: None,
                mode: None,
                identity: None,
            });
        }
        return Err(error.into());
    }
    let mut file = unsafe { fs::File::from_raw_fd(descriptor) };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_ADAPTER_FS_CONTENT_BYTES as u64 {
        return Err(adapter_conflict(
            "A managed Adapter leaf is not a bounded regular file.",
        ));
    }
    let mut content = Vec::with_capacity(metadata.len() as usize);
    std::io::Read::by_ref(&mut file)
        .take((MAX_ADAPTER_FS_CONTENT_BYTES + 1) as u64)
        .read_to_end(&mut content)?;
    if content.len() > MAX_ADAPTER_FS_CONTENT_BYTES {
        return Err(adapter_conflict(
            "A managed Adapter leaf grew beyond its supported size while being read.",
        ));
    }
    Ok(FileSnapshot {
        content: Some(content),
        mode: Some(metadata.mode() & 0o7777),
        identity: Some(FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        }),
    })
}

#[cfg(unix)]
fn create_staged_file_at(
    directory: &ManagedLeafDirectory,
    name: &std::ffi::CStr,
    stage_name: &std::ffi::CStr,
    content: &[u8],
    mode: u32,
) -> Result<(), DaemonError> {
    use std::os::fd::FromRawFd;

    let expected = file_snapshot_from_journal(&Some(content.to_vec()), Some(mode));
    let current = file_snapshot_at(directory, name)?;
    if current.content.is_some() {
        return if snapshots_match(&current, &expected) {
            Ok(())
        } else {
            Err(adapter_conflict(
                "A private Adapter staging file was replaced by an external writer.",
            ))
        };
    }

    // The random stage name was persisted in the FULL journal before any
    // filesystem mutation. A partial file therefore remains attributable to
    // this operation and can be removed/rebuilt after a crash without ever
    // guessing ownership of a user path.
    let stage = file_snapshot_at(directory, stage_name)?;
    if stage.content.is_some() {
        if snapshots_match(&stage, &expected) {
            return match rename_noreplace_at(directory, stage_name, name) {
                Ok(()) => directory.sync(),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let published = file_snapshot_at(directory, name)?;
                    if snapshots_match(&published, &expected) {
                        remove_file_at(directory, stage_name)
                    } else {
                        Err(adapter_conflict(
                            "A private Adapter staging file changed before publication.",
                        ))
                    }
                }
                Err(error) => Err(error.into()),
            };
        }
        remove_file_at(directory, stage_name)?;
    }
    let descriptor = unsafe {
        libc::openat(
            directory.descriptor(),
            stage_name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            mode & 0o7777,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut file = unsafe { fs::File::from_raw_fd(descriptor) };
    let write_result = (|| {
        file.write_all(content)?;
        let chmod_result =
            unsafe { libc::fchmod(file_descriptor(&file), (mode & 0o7777) as libc::mode_t) };
        if chmod_result != 0 {
            return Err(DaemonError::from(std::io::Error::last_os_error()));
        }
        file.sync_all()?;
        Ok(())
    })();
    drop(file);
    if let Err(error) = write_result {
        let _ = remove_file_at(directory, stage_name);
        return Err(error);
    }
    match rename_noreplace_at(directory, stage_name, name) {
        Ok(()) => directory.sync(),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let published = file_snapshot_at(directory, name)?;
            let _ = remove_file_at(directory, stage_name);
            if snapshots_match(&published, &expected) {
                Ok(())
            } else {
                Err(adapter_conflict(
                    "A private Adapter staging file changed before publication.",
                ))
            }
        }
        Err(error) => {
            let _ = remove_file_at(directory, stage_name);
            Err(error.into())
        }
    }
}

#[cfg(unix)]
fn file_descriptor(file: &fs::File) -> std::os::fd::RawFd {
    use std::os::fd::AsRawFd;
    file.as_raw_fd()
}

#[cfg(unix)]
fn rename_noreplace_at(
    directory: &ManagedLeafDirectory,
    source: &std::ffi::CStr,
    destination: &std::ffi::CStr,
) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            directory.descriptor(),
            source.as_ptr(),
            directory.descriptor(),
            destination.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            directory.descriptor(),
            source.as_ptr(),
            directory.descriptor(),
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let result = -1;
    if result == 0 {
        Ok(())
    } else {
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic no-replace rename is unsupported",
        ));
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn remove_file_at(
    directory: &ManagedLeafDirectory,
    name: &std::ffi::CStr,
) -> Result<(), DaemonError> {
    let result = unsafe { libc::unlinkat(directory.descriptor(), name.as_ptr(), 0) };
    if result == 0 {
        directory.sync()?;
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::NotFound {
        Ok(())
    } else {
        Err(error.into())
    }
}

#[cfg(unix)]
fn rename_noreplace_or_conflict(
    directory: &ManagedLeafDirectory,
    source: &std::ffi::CStr,
    destination: &std::ffi::CStr,
    conflict_message: &'static str,
) -> Result<(), DaemonError> {
    match rename_noreplace_at(directory, source, destination) {
        Ok(()) => {
            directory.sync()?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(adapter_conflict(conflict_message))
        }
        Err(error)
            if error.kind() == std::io::ErrorKind::Unsupported
                || matches!(
                    error.raw_os_error(),
                    Some(code)
                        if code == libc::ENOSYS
                            || code == libc::EINVAL
                            || code == libc::ENOTSUP
                ) =>
        {
            Err(state_error(
                "project_agent_adapter_atomic_rename_unsupported",
                "The repository filesystem does not support atomic no-replace renames required for a safe Adapter update.",
            ))
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn apply_journal_change_cas(
    operation: &PreparedAdapterFsOp,
    index: usize,
    change: &JournalChange,
) -> Result<(), DaemonError> {
    let path = journal_change_path(
        &operation.workspace_root,
        operation.adapter,
        change,
        operation.global,
    )?;
    let directory = ManagedLeafDirectory::open(&path)?;
    let (old_name, new_name) = journal_transient_names(operation, index)?;
    let before = file_snapshot_from_journal(&change.before_content, change.before_mode);
    let after = file_snapshot_from_journal(&change.after_content, change.after_mode);

    if let Some(content) = change.after_content.as_deref() {
        let stage_name = std::ffi::CString::new(change.stage_name.as_deref().ok_or_else(|| {
            DaemonError::InvalidConfig("journaled file has no stage name".to_owned())
        })?)
        .map_err(|_| DaemonError::InvalidConfig("journaled stage name is invalid".to_owned()))?;
        create_staged_file_at(
            &directory,
            &new_name,
            &stage_name,
            content,
            change.after_mode.ok_or_else(|| {
                DaemonError::InvalidConfig("journaled file has no target mode".to_owned())
            })?,
        )?;
    }

    let mut current = file_snapshot_at(&directory, &directory.target)?;
    let mut captured = file_snapshot_at(&directory, &old_name)?;
    if snapshots_match(&current, &after) {
        if captured.content.is_some() && !snapshots_match(&captured, &before) {
            return Err(adapter_conflict(
                "A captured Adapter file changed after publication.",
            ));
        }
        directory.revalidate()?;
        return Ok(());
    }

    if before.content.is_some() {
        if captured.content.is_none() {
            if !snapshots_match(&current, &before) {
                return Err(adapter_conflict(
                    "A managed Adapter file changed before its atomic capture.",
                ));
            }
            match rename_noreplace_at(&directory, &directory.target, &old_name) {
                Ok(()) => directory.sync()?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
            captured = file_snapshot_at(&directory, &old_name)?;
            if !snapshots_match(&captured, &before) {
                let target_now = file_snapshot_at(&directory, &directory.target)?;
                if target_now.content.is_none() {
                    rename_noreplace_or_conflict(
                        &directory,
                        &old_name,
                        &directory.target,
                        "An external file appeared while a raced Adapter leaf was being restored.",
                    )?;
                }
                return Err(adapter_conflict(
                    "A managed Adapter file changed during its atomic capture; the external bytes were preserved.",
                ));
            }
        } else if !snapshots_match(&captured, &before) {
            return Err(adapter_conflict(
                "A private captured Adapter file no longer matches its journal.",
            ));
        }
        current = file_snapshot_at(&directory, &directory.target)?;
        if current.content.is_some() && !snapshots_match(&current, &after) {
            return Err(adapter_conflict(
                "An external file appeared while an Adapter update was being published.",
            ));
        }
    } else {
        if captured.content.is_some() {
            return Err(adapter_conflict(
                "An unexpected captured file exists for a newly managed Adapter leaf.",
            ));
        }
        if current.content.is_some() && !snapshots_match(&current, &after) {
            return Err(adapter_conflict(
                "An external file appeared at a newly managed Adapter path.",
            ));
        }
    }

    if change.after_content.is_some() {
        current = file_snapshot_at(&directory, &directory.target)?;
        if current.content.is_none() {
            rename_noreplace_or_conflict(
                &directory,
                &new_name,
                &directory.target,
                "An external file appeared before the Adapter leaf could be published.",
            )?;
        }
    }
    current = file_snapshot_at(&directory, &directory.target)?;
    if !snapshots_match(&current, &after) {
        return Err(adapter_conflict(
            "The managed Adapter leaf does not match its prepared target state.",
        ));
    }
    directory.revalidate()?;
    Ok(())
}

#[cfg(not(unix))]
fn apply_journal_change_cas(
    _operation: &PreparedAdapterFsOp,
    _index: usize,
    _change: &JournalChange,
) -> Result<(), DaemonError> {
    Err(state_error(
        "project_agent_adapter_atomic_rename_unsupported",
        "This platform does not provide the atomic no-replace filesystem operations required for Adapter updates.",
    ))
}

#[cfg(unix)]
fn cleanup_journal_change_cas(
    operation: &PreparedAdapterFsOp,
    index: usize,
    change: &JournalChange,
) -> Result<(), DaemonError> {
    let path = journal_change_path(
        &operation.workspace_root,
        operation.adapter,
        change,
        operation.global,
    )?;
    let directory = ManagedLeafDirectory::open(&path)?;
    let (old_name, new_name) = journal_transient_names(operation, index)?;
    let before = file_snapshot_from_journal(&change.before_content, change.before_mode);
    let after = file_snapshot_from_journal(&change.after_content, change.after_mode);
    let current = file_snapshot_at(&directory, &directory.target)?;
    if !snapshots_match(&current, &after) {
        return Err(adapter_conflict(
            "A managed Adapter file changed before its transaction could be finalized.",
        ));
    }
    let old = file_snapshot_at(&directory, &old_name)?;
    if old.content.is_some() && !snapshots_match(&old, &before) {
        return Err(adapter_conflict(
            "A captured Adapter file changed before its transaction could be finalized.",
        ));
    }
    let new = file_snapshot_at(&directory, &new_name)?;
    if new.content.is_some() && !snapshots_match(&new, &after) {
        return Err(adapter_conflict(
            "A private Adapter staging file changed before finalization.",
        ));
    }
    remove_file_at(&directory, &old_name)?;
    remove_file_at(&directory, &new_name)?;
    if let Some(stage_name) = &change.stage_name {
        let stage_name = std::ffi::CString::new(stage_name.as_str()).map_err(|_| {
            DaemonError::InvalidConfig("journaled stage name is invalid".to_owned())
        })?;
        remove_file_at(&directory, &stage_name)?;
    }
    directory.revalidate()?;
    Ok(())
}

#[cfg(not(unix))]
fn cleanup_journal_change_cas(
    _operation: &PreparedAdapterFsOp,
    _index: usize,
    _change: &JournalChange,
) -> Result<(), DaemonError> {
    Ok(())
}

async fn finalize_prepared_adapter_fs_op(
    pool: &SqlitePool,
    operation: &PreparedAdapterFsOp,
) -> Result<Option<DaemonProjectAgentAdapter>, DaemonError> {
    validate_prepared_adapter_fs_op(operation)?;
    let workspace = operation.workspace_root.display().to_string();
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let binding_project: Option<String> = sqlx::query_scalar(
        "SELECT project_id
         FROM project_bindings
         WHERE server_url = $1 AND workspace_root = $2",
    )
    .bind(&operation.server_url)
    .bind(&workspace)
    .fetch_optional(&mut *tx)
    .await?;
    if binding_project.as_deref() != Some(operation.project_id.as_str()) {
        return Err(state_error(
            "project_binding_changed",
            "The Project binding changed before the Coding Agent integration transaction was finalized.",
        ));
    }

    let result = match operation.action {
        AdapterFsAction::Install => {
            let manifest_json = operation.manifest_json.as_deref().ok_or_else(|| {
                DaemonError::InvalidConfig("adapter install journal has no manifest".to_owned())
            })?;
            let next_revision = operation.next_revision.ok_or_else(|| {
                DaemonError::InvalidConfig(
                    "adapter install journal has no target revision".to_owned(),
                )
            })?;
            let rows_affected = if let Some(expected_revision) = operation.expected_revision {
                sqlx::query(
                    "UPDATE project_agent_adapters
                     SET project_id = $4, revision = $5, manifest_json = $6,
                         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     WHERE server_url = $1 AND workspace_root = $2 AND adapter = $3
                       AND revision = $7",
                )
                .bind(&operation.server_url)
                .bind(&workspace)
                .bind(operation.adapter.as_str())
                .bind(&operation.project_id)
                .bind(next_revision)
                .bind(manifest_json)
                .bind(expected_revision)
                .execute(&mut *tx)
                .await?
                .rows_affected()
            } else {
                sqlx::query(
                    "INSERT INTO project_agent_adapters (
                         server_url, workspace_root, project_id, adapter, revision, manifest_json
                     ) VALUES ($1, $2, $3, $4, $5, $6)
                     ON CONFLICT (server_url, workspace_root, adapter) DO NOTHING",
                )
                .bind(&operation.server_url)
                .bind(&workspace)
                .bind(&operation.project_id)
                .bind(operation.adapter.as_str())
                .bind(next_revision)
                .bind(manifest_json)
                .execute(&mut *tx)
                .await?
                .rows_affected()
            };
            if rows_affected != 1 {
                return Err(state_error(
                    "project_agent_adapter_changed",
                    "The Coding Agent integration revision changed before its filesystem transaction was finalized.",
                ));
            }
            let row = sqlx::query(
                "SELECT server_url, workspace_root, project_id, adapter, revision,
                        manifest_json, created_at, updated_at
                 FROM project_agent_adapters
                 WHERE server_url = $1 AND workspace_root = $2 AND adapter = $3",
            )
            .bind(&operation.server_url)
            .bind(&workspace)
            .bind(operation.adapter.as_str())
            .fetch_one(&mut *tx)
            .await?;
            Some(adapter_from_row(&row)?)
        }
        AdapterFsAction::Remove => {
            let expected_revision = operation.expected_revision.ok_or_else(|| {
                DaemonError::InvalidConfig(
                    "adapter removal journal has no expected revision".to_owned(),
                )
            })?;
            let deleted = sqlx::query(
                "DELETE FROM project_agent_adapters
                 WHERE server_url = $1 AND workspace_root = $2 AND adapter = $3
                   AND revision = $4",
            )
            .bind(&operation.server_url)
            .bind(&workspace)
            .bind(operation.adapter.as_str())
            .bind(expected_revision)
            .execute(&mut *tx)
            .await?;
            if deleted.rows_affected() != 1 {
                return Err(state_error(
                    "project_agent_adapter_changed",
                    "The Coding Agent integration revision changed before removal was finalized.",
                ));
            }
            None
        }
    };
    let deleted = sqlx::query("DELETE FROM adapter_fs_ops WHERE operation_id = $1")
        .bind(&operation.operation_id)
        .execute(&mut *tx)
        .await?;
    if deleted.rows_affected() != 1 {
        return Err(DaemonError::InvalidConfig(
            "adapter filesystem journal disappeared before finalization".to_owned(),
        ));
    }
    tx.commit().await?;
    Ok(result)
}

async fn recover_one_adapter_fs_op(
    pool: &SqlitePool,
    operation: &PreparedAdapterFsOp,
) -> Result<(), DaemonError> {
    let changes = apply_prepared_adapter_fs_op(operation)?;
    finalize_prepared_adapter_fs_op(pool, operation).await?;
    if operation.action == AdapterFsAction::Remove {
        cleanup_empty_adapter_directories(&changes, &operation.workspace_root);
    }
    Ok(())
}

pub(crate) async fn recover_pending_fs_ops(pool: &SqlitePool) -> Result<(), DaemonError> {
    global::recover(pool).await?;
    let operations = pending_adapter_fs_ops(pool).await?;
    for operation in operations {
        recover_one_adapter_fs_op(pool, &operation).await.map_err(|error| {
            tracing::error!(
                operation_id = %operation.operation_id,
                adapter = operation.adapter.as_str(),
                "adapter filesystem transaction recovery failed"
            );
            state_error(
                "project_agent_adapter_recovery_conflict",
                &format!(
                    "A prepared Coding Agent integration transaction could not be recovered safely: {error}"
                ),
            )
        })?;
    }
    Ok(())
}

async fn recover_pending_fs_op_for_adapter(
    pool: &SqlitePool,
    server_url: &str,
    workspace_root: &Path,
    adapter: ProjectAgentAdapterKind,
) -> Result<(), DaemonError> {
    let row = sqlx::query(
        "SELECT operation_id, server_url, workspace_root, project_id, adapter,
                action, expected_revision, next_revision, manifest_json, changes_json
         FROM adapter_fs_ops
         WHERE server_url = $1 AND workspace_root = $2 AND adapter = $3",
    )
    .bind(server_url)
    .bind(workspace_root.display().to_string())
    .bind(adapter.as_str())
    .fetch_optional(pool)
    .await?;
    if let Some(row) = row {
        let operation = prepared_adapter_fs_op_from_row(&row)?;
        recover_one_adapter_fs_op(pool, &operation).await?;
    }
    Ok(())
}

pub(crate) async fn recover_pending_fs_ops_for_workspace(
    pool: &SqlitePool,
    server_url: &str,
    workspace_root: &Path,
) -> Result<(), DaemonError> {
    let rows = sqlx::query(
        "SELECT operation_id, server_url, workspace_root, project_id, adapter,
                action, expected_revision, next_revision, manifest_json, changes_json
         FROM adapter_fs_ops
         WHERE server_url = $1 AND workspace_root = $2
         ORDER BY created_at, operation_id",
    )
    .bind(server_url)
    .bind(workspace_root.display().to_string())
    .fetch_all(pool)
    .await?;
    for row in rows {
        let operation = prepared_adapter_fs_op_from_row(&row)?;
        recover_one_adapter_fs_op(pool, &operation).await?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) async fn has_pending_fs_ops(pool: &SqlitePool) -> Result<bool, DaemonError> {
    let pending: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM adapter_fs_ops LIMIT 1)")
        .fetch_one(pool)
        .await?;
    Ok(pending != 0)
}

pub(crate) async fn list(
    state: &DaemonState,
    request: DaemonProjectAgentAdapterListRequest,
) -> Result<DaemonProjectAgentAdapterListResponse, DaemonError> {
    let project_id = required_value("project_id", request.project_id)?;
    let server_url = canonical_server_url(&state.project_config().server_url)?;
    let rows = sqlx::query(
        "SELECT server_url, workspace_root, project_id, adapter, revision,
                manifest_json, created_at, updated_at
         FROM project_agent_adapters
         WHERE server_url = $1 AND project_id = $2
         ORDER BY workspace_root, adapter",
    )
    .bind(&server_url)
    .bind(&project_id)
    .fetch_all(&state.inner.pool)
    .await?;
    Ok(DaemonProjectAgentAdapterListResponse {
        items: rows
            .iter()
            .map(adapter_from_row)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

pub(crate) async fn list_all(
    state: &DaemonState,
) -> Result<DaemonProjectAgentAdapterListResponse, DaemonError> {
    let server_url = canonical_server_url(&state.project_config().server_url)?;
    let rows = sqlx::query(
        "SELECT server_url, workspace_root, project_id, adapter, revision,
                manifest_json, created_at, updated_at
         FROM project_agent_adapters
         WHERE server_url = $1
         ORDER BY workspace_root, adapter",
    )
    .bind(&server_url)
    .fetch_all(&state.inner.pool)
    .await?;
    Ok(DaemonProjectAgentAdapterListResponse {
        items: rows
            .iter()
            .map(adapter_from_row)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

pub(crate) async fn require_runtime_delivery(
    state: &DaemonState,
    binding: &crate::DaemonProjectBinding,
    required: crate::ProjectAgentAdapterRuntimeRequirement,
) -> Result<(), DaemonError> {
    if let Some(enabled) = global::enabled(&state.inner.pool, required.adapter).await? {
        let delivery_matches = required.delivery
            == if required.adapter == ProjectAgentAdapterKind::Codex {
                ProjectAgentAdapterDelivery::HostPlugin
            } else {
                ProjectAgentAdapterDelivery::LegacyFiles
            };
        return if enabled && delivery_matches {
            Ok(())
        } else {
            Err(state_error(
                "agent_adapter_disabled",
                "This Agent integration is disabled on this Mac.",
            ))
        };
    }
    if required.adapter == ProjectAgentAdapterKind::Codex
        && required.delivery == ProjectAgentAdapterDelivery::HostPlugin
    {
        return Ok(());
    }
    let raw_manifest: Option<String> = sqlx::query_scalar(
        "SELECT manifest_json
         FROM project_agent_adapters
         WHERE server_url = $1 AND workspace_root = $2 AND project_id = $3 AND adapter = $4",
    )
    .bind(&binding.server_url)
    .bind(&binding.workspace_root)
    .bind(&binding.project_id)
    .bind(required.adapter.as_str())
    .fetch_optional(&state.inner.pool)
    .await?;
    let enabled = raw_manifest
        .as_deref()
        .map(serde_json::from_str::<AdapterManifest>)
        .transpose()?
        .is_some_and(|manifest| manifest.delivery == required.delivery);
    if !enabled {
        return Err(state_error(
            "project_agent_adapter_not_enabled",
            "The requested Coding Agent integration is not enabled for this workspace.",
        ));
    }
    Ok(())
}

pub(crate) async fn inspect_legacy(
    state: &DaemonState,
    request: DaemonLegacyAgentAdapterInspectionRequest,
) -> Result<DaemonLegacyAgentAdapterInspectionResponse, DaemonError> {
    legacy::inspect(state, request).await
}

pub(crate) async fn inspect_codex_plugin(
    state: &DaemonState,
    request: DaemonCodexPluginRequest,
) -> Result<DaemonCodexPluginStatus, DaemonError> {
    let runtime_binary = canonical_agent_runtime_binary(&request.runtime_binary_path)?;
    #[cfg(target_os = "macos")]
    verify_code_signature(&runtime_binary)?;
    let runtime_hash = sha256_file(&runtime_binary)?;
    codex_plugin::inspect(
        &state.inner.config.root_dir,
        &runtime_binary,
        &runtime_hash,
        request.host_binary_path.as_deref(),
        state.inner.config.dev_instance_id.as_deref(),
    )
    .await
}

pub(crate) async fn reconcile_codex_plugin(
    state: &DaemonState,
    request: DaemonCodexPluginRequest,
) -> Result<DaemonCodexPluginStatus, DaemonError> {
    let _guard = state.inner.local_setup_lock.lock().await;
    let runtime_binary = canonical_agent_runtime_binary(&request.runtime_binary_path)?;
    #[cfg(target_os = "macos")]
    verify_code_signature(&runtime_binary)?;
    let runtime_hash = sha256_file(&runtime_binary)?;
    codex_plugin::ensure_installed(
        &state.inner.config.root_dir,
        &runtime_binary,
        &runtime_hash,
        request.host_binary_path.as_deref(),
        state.inner.config.dev_instance_id.as_deref(),
    )
    .await?;
    codex_plugin::inspect(
        &state.inner.config.root_dir,
        &runtime_binary,
        &runtime_hash,
        request.host_binary_path.as_deref(),
        state.inner.config.dev_instance_id.as_deref(),
    )
    .await
}

pub(crate) async fn install(
    state: &DaemonState,
    request: DaemonProjectAgentAdapterInstallRequest,
) -> Result<DaemonProjectAgentAdapter, DaemonError> {
    if request.adapter == ProjectAgentAdapterKind::Codex {
        return Err(DaemonError::InvalidRequest(
            "The Codex Plugin is managed globally and cannot be installed as a repository integration."
                .to_owned(),
        ));
    }
    let _guard = state.inner.local_setup_lock.lock().await;
    if global::enabled(&state.inner.pool, request.adapter)
        .await?
        .is_some()
    {
        return Err(DaemonError::InvalidRequest(
            "This harness is managed globally; use Settings → Agents.".to_owned(),
        ));
    }

    let project_id = required_value("project_id", request.project_id)?;
    let workspace_root = canonical_workspace_directory(&request.workspace_root)?;
    let server_url = canonical_server_url(&state.project_config().server_url)?;
    normalize_empty_binding_alias(&state.inner.pool, &server_url, &workspace_root, &project_id)
        .await?;
    recover_pending_fs_op_for_adapter(
        &state.inner.pool,
        &server_url,
        &workspace_root,
        request.adapter,
    )
    .await?;
    ensure_binding(&state.inner.pool, &server_url, &workspace_root, &project_id).await?;

    let existing_row = sqlx::query(
        "SELECT server_url, workspace_root, project_id, adapter, revision,
                manifest_json, created_at, updated_at
         FROM project_agent_adapters
         WHERE server_url = $1 AND workspace_root = $2 AND adapter = $3",
    )
    .bind(&server_url)
    .bind(workspace_root.display().to_string())
    .bind(request.adapter.as_str())
    .fetch_optional(&state.inner.pool)
    .await?;
    let existing = existing_row
        .as_ref()
        .map(adapter_record_from_row)
        .transpose()?;
    if request.expected_revision.is_some()
        && existing.as_ref().map(|record| record.status.revision) != request.expected_revision
    {
        return Err(state_error(
            "project_agent_adapter_changed",
            "The Coding Agent integration changed from the expected revision.",
        ));
    }

    // Keep the stable App-bundled clumsiesd path in rendered adapter files
    // instead of copying a second executable into application support.
    let runtime_binary = canonical_agent_runtime_binary(&request.runtime_binary_path)?;
    #[cfg(target_os = "macos")]
    verify_code_signature(&runtime_binary)?;
    let runtime_hash = sha256_file(&runtime_binary)?;
    let previous_manifest = existing.as_ref().map(|record| &record.manifest);
    let mut changes = install_plan_with_project(
        request.adapter,
        &workspace_root,
        &runtime_binary,
        previous_manifest,
        Some(&server_url),
        Some(&project_id),
    )?;
    // Retire previously-managed files the new plan no longer includes
    // instead of silently orphaning them on disk.
    changes.extend(retire_stale_managed_changes(
        &changes,
        previous_manifest,
        &workspace_root,
    )?);
    let manifest = manifest_for_changes(&changes, &runtime_binary, runtime_hash);
    let files_changed = changes
        .iter()
        .map(change_is_needed)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .any(|changed| changed);
    let next_revision = match &existing {
        Some(record) if !files_changed && record.manifest == manifest => record.status.revision,
        Some(record) => record.status.revision + 1,
        None => 1,
    };
    if let Some(record) = &existing
        && next_revision == record.status.revision
    {
        return Ok(record.status.clone());
    }
    let manifest_json = serde_json::to_string(&manifest)?;
    let operation = prepared_adapter_fs_op(
        PreparedAdapterFsOp {
            global: false,
            operation_id: Uuid::new_v4().to_string(),
            server_url,
            workspace_root: workspace_root.clone(),
            project_id,
            adapter: request.adapter,
            action: AdapterFsAction::Install,
            expected_revision: existing.as_ref().map(|record| record.status.revision),
            next_revision: Some(next_revision),
            manifest_json: Some(manifest_json),
            changes: Vec::new(),
        },
        &changes,
    )?;
    persist_prepared_adapter_fs_op(&state.inner.pool, &operation).await?;
    apply_prepared_adapter_fs_op(&operation)?;
    let adapter = finalize_prepared_adapter_fs_op(&state.inner.pool, &operation)
        .await?
        .ok_or_else(|| {
            DaemonError::InvalidConfig(
                "adapter install transaction finalized without a manifest".to_owned(),
            )
        })?;
    cleanup_empty_adapter_directories(&changes, &workspace_root);
    Ok(adapter)
}

pub(crate) async fn remove(
    state: &DaemonState,
    request: DaemonProjectAgentAdapterRemoveRequest,
) -> Result<DaemonProjectAgentAdapterRemoveResponse, DaemonError> {
    let _guard = state.inner.local_setup_lock.lock().await;
    let server_url = canonical_server_url(&state.project_config().server_url)?;
    remove_from_server(state, request, server_url).await
}

async fn remove_from_server(
    state: &DaemonState,
    request: DaemonProjectAgentAdapterRemoveRequest,
    server_url: String,
) -> Result<DaemonProjectAgentAdapterRemoveResponse, DaemonError> {
    let workspace_root = canonical_workspace_directory(&request.workspace_root)?;
    recover_pending_fs_op_for_adapter(
        &state.inner.pool,
        &server_url,
        &workspace_root,
        request.adapter,
    )
    .await?;
    let row = sqlx::query(
        "SELECT server_url, workspace_root, project_id, adapter, revision,
                manifest_json, created_at, updated_at
         FROM project_agent_adapters
         WHERE server_url = $1 AND workspace_root = $2 AND adapter = $3",
    )
    .bind(&server_url)
    .bind(workspace_root.display().to_string())
    .bind(request.adapter.as_str())
    .fetch_optional(&state.inner.pool)
    .await?
    .ok_or_else(|| {
        state_error(
            "project_agent_adapter_not_found",
            "The Coding Agent integration is not installed for this repository.",
        )
    })?;
    let existing = adapter_record_from_row(&row)?;
    if existing.status.revision != request.expected_revision {
        return Err(state_error(
            "project_agent_adapter_changed",
            "The Coding Agent integration changed from the expected revision.",
        ));
    }

    let changes = remove_plan(&existing.manifest, &workspace_root)?;
    let operation = prepared_adapter_fs_op(
        PreparedAdapterFsOp {
            global: false,
            operation_id: Uuid::new_v4().to_string(),
            server_url,
            workspace_root: workspace_root.clone(),
            project_id: existing.status.project_id.clone(),
            adapter: request.adapter,
            action: AdapterFsAction::Remove,
            expected_revision: Some(request.expected_revision),
            next_revision: None,
            manifest_json: None,
            changes: Vec::new(),
        },
        &changes,
    )?;
    persist_prepared_adapter_fs_op(&state.inner.pool, &operation).await?;
    apply_prepared_adapter_fs_op(&operation)?;
    finalize_prepared_adapter_fs_op(&state.inner.pool, &operation).await?;
    cleanup_empty_adapter_directories(&changes, &workspace_root);
    Ok(DaemonProjectAgentAdapterRemoveResponse {
        workspace_root: workspace_root.display().to_string(),
        adapter: request.adapter,
        removed: true,
    })
}

async fn ensure_binding(
    pool: &SqlitePool,
    server_url: &str,
    workspace_root: &Path,
    project_id: &str,
) -> Result<(), DaemonError> {
    let row = sqlx::query(
        "SELECT server_url, workspace_root, project_id, revision, created_at, updated_at
         FROM project_bindings
         WHERE server_url = $1 AND workspace_root = $2",
    )
    .bind(server_url)
    .bind(workspace_root.display().to_string())
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        state_error(
            "project_binding_not_found",
            "Bind this repository to the Project before installing a Coding Agent integration.",
        )
    })?;
    let binding = project_binding_from_row(&row)?;
    if binding.project_id != project_id {
        return Err(state_error(
            "project_binding_changed",
            "This repository is bound to a different Project.",
        ));
    }
    Ok(())
}

/// Rekeys a historical binding whose stored lexical path now resolves to the
/// canonical workspace path used by adapter filesystem transactions.
///
/// Adapter journals and their foreign key deliberately use one exact workspace
/// key. Moving an occupied binding would also require rebasing its manifests
/// and absolute hook commands, so this narrow repair is allowed only before the
/// first daemon-owned adapter (and before any pending filesystem transaction).
async fn normalize_empty_binding_alias(
    pool: &SqlitePool,
    server_url: &str,
    workspace_root: &Path,
    project_id: &str,
) -> Result<(), DaemonError> {
    let canonical_workspace = workspace_root.display().to_string();
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let rows = sqlx::query(
        "SELECT server_url, workspace_root, project_id, revision, created_at, updated_at
         FROM project_bindings
         WHERE server_url = $1",
    )
    .bind(server_url)
    .fetch_all(&mut *tx)
    .await?;

    let mut equivalents = rows
        .iter()
        .map(project_binding_from_row)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|binding| canonical_binding_root(&binding.workspace_root) == workspace_root)
        .collect::<Vec<_>>();
    equivalents.sort_by(|left, right| left.workspace_root.cmp(&right.workspace_root));

    if equivalents.len() > 1 {
        return Err(state_error(
            "project_binding_changed",
            "More than one stored Project binding resolves to this repository. Remove the duplicate binding before installing a Coding Agent integration.",
        ));
    }
    let Some(binding) = equivalents.pop() else {
        tx.commit().await?;
        return Ok(());
    };
    if binding.project_id != project_id {
        return Err(state_error(
            "project_binding_changed",
            "This repository is bound to a different Project.",
        ));
    }
    if binding.workspace_root == canonical_workspace {
        tx.commit().await?;
        return Ok(());
    }

    let adapter_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM project_agent_adapters
         WHERE server_url = $1 AND workspace_root = $2",
    )
    .bind(server_url)
    .bind(&binding.workspace_root)
    .fetch_one(&mut *tx)
    .await?;
    let pending_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM adapter_fs_ops
         WHERE server_url = $1 AND workspace_root = $2",
    )
    .bind(server_url)
    .bind(&binding.workspace_root)
    .fetch_one(&mut *tx)
    .await?;
    if adapter_count != 0 || pending_count != 0 {
        return Err(state_error(
            "project_agent_adapter_recovery_required",
            "This repository moved after a Coding Agent integration was installed. Remove or recover the existing integration before changing its stored path.",
        ));
    }

    let inserted = sqlx::query(
        "INSERT INTO project_bindings (
             server_url, workspace_root, project_id, revision, created_at, updated_at
         )
         SELECT server_url, $3, project_id, revision, created_at, updated_at
         FROM project_bindings
         WHERE server_url = $1 AND workspace_root = $2
           AND project_id = $4 AND revision = $5",
    )
    .bind(server_url)
    .bind(&binding.workspace_root)
    .bind(&canonical_workspace)
    .bind(project_id)
    .bind(binding.revision)
    .execute(&mut *tx)
    .await?;
    if inserted.rows_affected() != 1 {
        return Err(state_error(
            "project_binding_changed",
            "The Project binding changed while its repository path was being normalized.",
        ));
    }
    let deleted = sqlx::query(
        "DELETE FROM project_bindings
         WHERE server_url = $1 AND workspace_root = $2
           AND project_id = $3 AND revision = $4",
    )
    .bind(server_url)
    .bind(&binding.workspace_root)
    .bind(project_id)
    .bind(binding.revision)
    .execute(&mut *tx)
    .await?;
    if deleted.rows_affected() != 1 {
        return Err(state_error(
            "project_binding_changed",
            "The Project binding changed while its repository path was being normalized.",
        ));
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
fn install_plan(
    adapter: ProjectAgentAdapterKind,
    workspace_root: &Path,
    runtime_binary: &Path,
    previous_manifest: Option<&AdapterManifest>,
) -> Result<Vec<PendingChange>, DaemonError> {
    install_plan_with_project(
        adapter,
        workspace_root,
        runtime_binary,
        previous_manifest,
        None,
        None,
    )
}

fn install_plan_with_project(
    adapter: ProjectAgentAdapterKind,
    workspace_root: &Path,
    runtime_binary: &Path,
    previous_manifest: Option<&AdapterManifest>,
    server_url: Option<&str>,
    project_id: Option<&str>,
) -> Result<Vec<PendingChange>, DaemonError> {
    install_plan_with_claude_mcp_path(
        adapter,
        workspace_root,
        runtime_binary,
        previous_manifest,
        None,
        server_url,
        project_id,
    )
}

fn install_plan_with_claude_mcp_path(
    adapter: ProjectAgentAdapterKind,
    workspace_root: &Path,
    runtime_binary: &Path,
    previous_manifest: Option<&AdapterManifest>,
    claude_mcp_path: Option<&Path>,
    server_url: Option<&str>,
    project_id: Option<&str>,
) -> Result<Vec<PendingChange>, DaemonError> {
    let effective_claude_mcp_path = claude_mcp_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| workspace_root.join(".mcp.json"));
    let target_paths = match adapter {
        ProjectAgentAdapterKind::Codex => vec![
            workspace_root.join(".codex/config.toml"),
            workspace_root.join(".codex/hooks.json"),
            workspace_root.join(".codex/hooks/resolve-binary.sh"),
            workspace_root.join(".codex/hooks/agent-run-event.sh"),
            workspace_root.join(".codex/hooks/user-prompt-submit.sh"),
        ],
        ProjectAgentAdapterKind::ClaudeCode => vec![
            effective_claude_mcp_path.clone(),
            workspace_root.join(".claude/settings.json"),
            workspace_root.join(".claude/hooks/resolve-binary.sh"),
            workspace_root.join(".claude/hooks/agent-run-event.sh"),
            workspace_root.join(".claude/hooks/user-prompt-submit.sh"),
        ],
        ProjectAgentAdapterKind::Opencode => vec![
            workspace_root.join("opencode.json"),
            workspace_root.join(".opencode/plugins/clumsies.ts"),
        ],
        ProjectAgentAdapterKind::Dsh => vec![workspace_root.join(".dsh/clumsies.json")],
        ProjectAgentAdapterKind::Antigravity => vec![
            workspace_root.join(".mcp.json"),
            workspace_root.join(".agents/hooks.json"),
            workspace_root.join(".agents/hooks/resolve-binary.sh"),
            workspace_root.join(".agents/hooks/agent-run-event.sh"),
        ],
    };
    for path in &target_paths {
        ManagedPathGuard::capture_under(workspace_root, path)?;
    }

    let runtime = runtime_binary.display().to_string();
    let previous_runtime = previous_manifest.map(|manifest| manifest.runtime_binary_path.as_str());
    let mut changes = match adapter {
        ProjectAgentAdapterKind::Codex => {
            let hooks_path = workspace_root.join(".codex/hooks.json");
            let hook_script_path = workspace_root.join(".codex/hooks/agent-run-event.sh");
            let legacy_hook_script_path = workspace_root.join(".codex/hooks/user-prompt-submit.sh");
            let hook_ownership = HookOwnership {
                lifecycle: manifest_manages_path(previous_manifest, &hook_script_path),
                legacy_prompt: legacy_hook_is_proven_managed(
                    previous_manifest,
                    &legacy_hook_script_path,
                    LEGACY_USER_PROMPT_SUBMIT_CODEX_SHA256,
                )?,
            };
            let managed_hook = render_managed_hook_script(ISSUE_RUN_EVENT_CODEX, &runtime);
            let managed_resolver =
                render_managed_binary_resolver(ProjectAgentAdapterKind::Codex, &runtime);
            vec![
                merged_change(
                    workspace_root.join(".codex/config.toml"),
                    ManagedFileKind::CodexConfig,
                    0o644,
                    |current| render_codex_config(current, &runtime, previous_runtime),
                )?,
                merged_change(
                    hooks_path.clone(),
                    ManagedFileKind::CodexHooks,
                    0o644,
                    |current| {
                        render_hook_registry(current, &hook_script_path, false, hook_ownership)
                    },
                )?,
                exclusive_change(
                    workspace_root.join(".codex/hooks/resolve-binary.sh"),
                    managed_resolver.as_bytes(),
                    previous_manifest,
                    0o755,
                )?,
                exclusive_change(
                    hook_script_path,
                    managed_hook.as_bytes(),
                    previous_manifest,
                    0o755,
                )?,
            ]
        }
        ProjectAgentAdapterKind::ClaudeCode => {
            let settings_path = workspace_root.join(".claude/settings.json");
            let mcp_path = effective_claude_mcp_path;
            let hook_script_path = workspace_root.join(".claude/hooks/agent-run-event.sh");
            let legacy_hook_script_path =
                workspace_root.join(".claude/hooks/user-prompt-submit.sh");
            let hook_ownership = HookOwnership {
                lifecycle: manifest_manages_path(previous_manifest, &hook_script_path),
                legacy_prompt: legacy_hook_is_proven_managed(
                    previous_manifest,
                    &legacy_hook_script_path,
                    LEGACY_USER_PROMPT_SUBMIT_CLAUDE_SHA256,
                )?,
            };
            let managed_hook = render_managed_hook_script(ISSUE_RUN_EVENT_CLAUDE, &runtime);
            let managed_resolver =
                render_managed_binary_resolver(ProjectAgentAdapterKind::ClaudeCode, &runtime);
            vec![
                merged_change(
                    mcp_path.clone(),
                    ManagedFileKind::ClaudeMcp,
                    0o644,
                    |current| render_claude_mcp(current, &runtime, previous_runtime),
                )?,
                merged_change(
                    settings_path.clone(),
                    ManagedFileKind::ClaudeSettings,
                    0o644,
                    |current| {
                        render_hook_registry(current, &hook_script_path, true, hook_ownership)
                    },
                )?,
                exclusive_change(
                    workspace_root.join(".claude/hooks/resolve-binary.sh"),
                    managed_resolver.as_bytes(),
                    previous_manifest,
                    0o755,
                )?,
                exclusive_change(
                    hook_script_path,
                    managed_hook.as_bytes(),
                    previous_manifest,
                    0o755,
                )?,
            ]
        }
        ProjectAgentAdapterKind::Opencode => {
            let config_path = workspace_root.join("opencode.json");
            let plugin_path = workspace_root.join(".opencode/plugins/clumsies.ts");
            vec![
                merged_change(
                    config_path.clone(),
                    ManagedFileKind::OpencodeConfig,
                    0o644,
                    |current| render_opencode_config(current, &runtime, previous_runtime),
                )?,
                exclusive_change(
                    plugin_path,
                    render_opencode_plugin(&runtime).as_bytes(),
                    previous_manifest,
                    0o644,
                )?,
            ]
        }
        ProjectAgentAdapterKind::Dsh => {
            let server_url = server_url.ok_or_else(|| {
                adapter_conflict("The dsh adapter plan needs the daemon server URL.")
            })?;
            let project_id = project_id.ok_or_else(|| {
                adapter_conflict("The dsh adapter plan needs the bound Project id.")
            })?;
            vec![exclusive_change_with_kind(
                workspace_root.join(".dsh/clumsies.json"),
                &render_dsh_config(server_url, project_id, &runtime),
                previous_manifest,
                0o644,
                ManagedFileKind::DshConfig,
            )?]
        }
        ProjectAgentAdapterKind::Antigravity => {
            let mcp_path = workspace_root.join(".mcp.json");
            let hooks_path = workspace_root.join(".agents/hooks.json");
            let hook_script_path = workspace_root.join(".agents/hooks/agent-run-event.sh");
            let hook_ownership = HookOwnership {
                lifecycle: manifest_manages_path(previous_manifest, &hook_script_path),
                legacy_prompt: false,
            };
            let managed_hook = render_managed_hook_script(ISSUE_RUN_EVENT_ANTIGRAVITY, &runtime);
            let managed_resolver =
                render_managed_binary_resolver(ProjectAgentAdapterKind::Antigravity, &runtime);
            vec![
                merged_change(mcp_path, ManagedFileKind::ClaudeMcp, 0o644, |current| {
                    render_claude_mcp(current, &runtime, previous_runtime)
                })?,
                merged_change(
                    hooks_path.clone(),
                    ManagedFileKind::AntigravityHooks,
                    0o644,
                    |current| render_antigravity_hooks(current, &hook_script_path, hook_ownership),
                )?,
                exclusive_change(
                    workspace_root.join(".agents/hooks/resolve-binary.sh"),
                    managed_resolver.as_bytes(),
                    previous_manifest,
                    0o755,
                )?,
                exclusive_change(
                    hook_script_path,
                    managed_hook.as_bytes(),
                    previous_manifest,
                    0o755,
                )?,
            ]
        }
    };
    changes.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(changes)
}

/// Renders the workspace marker the dsh hook plugin reads to route dsh
/// sessions to this Project: the daemon's canonical server URL, the bound
/// Project id, and the App-bundled clumsiesd path that forwards lifecycle
/// events. The file is fully owned by this integration (Exclusive).
fn render_dsh_config(server_url: &str, project_id: &str, runtime_binary: &str) -> Vec<u8> {
    let value = json!({
        "server_url": server_url,
        "project_id": project_id,
        "runtime": runtime_binary,
    });
    let mut rendered = serde_json::to_vec_pretty(&value)
        .expect("serializing the dsh adapter config to JSON cannot fail");
    rendered.push(b'\n');
    rendered
}

/// Retires previously-managed exclusive files the new install plan no
/// longer includes. The thin-skills retirement (ISSUE-064) relies on this to
/// delete stale `.agents/skills/*` and `.claude/skills/*` files on the next
/// adapter update instead of silently orphaning them. A file that changed
/// since install is a conflict, mirroring `remove_plan` semantics.
fn retire_stale_managed_changes(
    changes: &[PendingChange],
    previous_manifest: Option<&AdapterManifest>,
    workspace_root: &Path,
) -> Result<Vec<PendingChange>, DaemonError> {
    let Some(previous_manifest) = previous_manifest else {
        return Ok(Vec::new());
    };
    let plan_paths: BTreeSet<&Path> = changes.iter().map(|change| change.path.as_path()).collect();
    let mut retired = Vec::new();
    for file in &previous_manifest.managed_files {
        let path = PathBuf::from(&file.path);
        if plan_paths.contains(path.as_path()) || file.kind != ManagedFileKind::Exclusive {
            continue;
        }
        validate_manifest_managed_path(workspace_root, &path, file.kind)?;
        let expected = capture_file_snapshot(&path)?;
        if let Some(content) = &expected.content
            && sha256(content) != file.installed_hash
        {
            return Err(state_error(
                "project_agent_adapter_conflict",
                &format!(
                    "{} changed after Clumsies installed it; review it before updating the integration.",
                    path.display()
                ),
            ));
        }
        retired.push(PendingChange {
            path,
            expected,
            desired: None,
            kind: file.kind,
            mode: 0o644,
        });
    }
    Ok(retired)
}

fn remove_plan(
    manifest: &AdapterManifest,
    workspace_root: &Path,
) -> Result<Vec<PendingChange>, DaemonError> {
    let helper = &manifest.runtime_binary_path;
    let mut seen = BTreeSet::new();
    for file in &manifest.managed_files {
        let path = PathBuf::from(&file.path);
        if !seen.insert(path.clone()) {
            return Err(adapter_conflict(&format!(
                "The adapter manifest contains the managed path {} more than once.",
                path.display()
            )));
        }
        validate_manifest_managed_path(workspace_root, &path, file.kind)?;
    }
    manifest
        .managed_files
        .iter()
        .map(|file| {
            let path = PathBuf::from(&file.path);
            let expected = capture_file_snapshot(&path)?;
            let desired = match file.kind {
                ManagedFileKind::CodexConfig => expected
                    .content
                    .as_deref()
                    .map(|content| remove_codex_config(content, helper))
                    .transpose()?
                    .flatten(),
                ManagedFileKind::CodexHooks => expected
                    .content
                    .as_deref()
                    .map(|content| {
                        remove_hook_registry(
                            content,
                            &path
                                .parent()
                                .unwrap_or(Path::new("."))
                                .join("hooks/agent-run-event.sh"),
                            false,
                        )
                    })
                    .transpose()?
                    .flatten(),
                ManagedFileKind::ClaudeMcp => expected
                    .content
                    .as_deref()
                    .map(|content| remove_claude_mcp(content, helper))
                    .transpose()?
                    .flatten(),
                ManagedFileKind::ClaudeSettings => expected
                    .content
                    .as_deref()
                    .map(|content| {
                        remove_hook_registry(
                            content,
                            &path
                                .parent()
                                .unwrap_or(Path::new("."))
                                .join("hooks/agent-run-event.sh"),
                            true,
                        )
                    })
                    .transpose()?
                    .flatten(),
                ManagedFileKind::OpencodeConfig => expected
                    .content
                    .as_deref()
                    .map(|content| {
                        if path == workspace_root.join(".config/opencode/opencode.json") {
                            remove_opencode_config_for_scope(content, helper, false)
                        } else {
                            remove_opencode_config(content, helper)
                        }
                    })
                    .transpose()?
                    .flatten(),
                ManagedFileKind::AntigravityHooks => expected
                    .content
                    .as_deref()
                    .map(|content| {
                        remove_antigravity_hooks(
                            content,
                            &path
                                .parent()
                                .unwrap_or(Path::new("."))
                                .join("hooks/agent-run-event.sh"),
                        )
                    })
                    .transpose()?
                    .flatten(),
                ManagedFileKind::DshConfig | ManagedFileKind::Exclusive => {
                    if let Some(content) = &expected.content
                        && sha256(content) != file.installed_hash {
                            return Err(state_error(
                                "project_agent_adapter_conflict",
                                &format!(
                                    "{} changed after Clumsies installed it; review it before removing the integration.",
                                    path.display()
                                ),
                            ));
                        }
                    None
                }
            };
            Ok(PendingChange {
                path,
                expected,
                desired,
                kind: file.kind,
                mode: 0o644,
            })
        })
        .collect()
}

fn validate_manifest_managed_path(
    workspace_root: &Path,
    path: &Path,
    kind: ManagedFileKind,
) -> Result<(), DaemonError> {
    ManagedPathGuard::capture_under(workspace_root, path)?;
    let relative = path.strip_prefix(workspace_root).map_err(|_| {
        adapter_conflict(&format!(
            "Managed path {} escapes its adapter workspace.",
            path.display()
        ))
    })?;
    let allowed = match kind {
        ManagedFileKind::CodexConfig => relative == Path::new(".codex/config.toml"),
        ManagedFileKind::CodexHooks => relative == Path::new(".codex/hooks.json"),
        ManagedFileKind::ClaudeMcp => {
            relative == Path::new(".mcp.json")
                || relative == Path::new(".claude.json")
                || relative == Path::new(".gemini/config/mcp_config.json")
        }
        ManagedFileKind::ClaudeSettings => relative == Path::new(".claude/settings.json"),
        ManagedFileKind::OpencodeConfig => matches!(
            relative.to_str(),
            Some("opencode.json" | ".config/opencode/opencode.json")
        ),
        ManagedFileKind::DshConfig => relative == Path::new(".dsh/clumsies.json"),
        ManagedFileKind::AntigravityHooks => matches!(
            relative.to_str(),
            Some(".agents/hooks.json" | ".gemini/config/hooks.json")
        ),
        // The retired thin-skill paths stay accepted so that manifests and
        // pending journal ops written before the thin-skills retirement
        // (ISSUE-064) remain removable and recoverable. New plans never
        // produce them; the update path retires the files on disk.
        ManagedFileKind::Exclusive => [
            ".codex/hooks/resolve-binary.sh",
            ".codex/hooks/agent-run-event.sh",
            ".codex/hooks/user-prompt-submit.sh",
            ".agents/skills/activate/SKILL.md",
            ".agents/skills/ntmd/SKILL.md",
            ".claude/hooks/resolve-binary.sh",
            ".claude/hooks/agent-run-event.sh",
            ".claude/hooks/user-prompt-submit.sh",
            ".claude/skills/activate/SKILL.md",
            ".claude/skills/ntmd/SKILL.md",
            ".opencode/plugins/clumsies.ts",
            ".config/opencode/plugins/clumsies.ts",
            ".gemini/config/hooks/resolve-binary.sh",
            ".gemini/config/hooks/agent-run-event.sh",
            ".agents/hooks/resolve-binary.sh",
            ".agents/hooks/agent-run-event.sh",
        ]
        .iter()
        .any(|candidate| relative == Path::new(candidate)),
    };
    if !allowed {
        return Err(adapter_conflict(&format!(
            "Managed path {} is outside the fixed namespace for its adapter resource kind.",
            path.display()
        )));
    }
    Ok(())
}

fn exclusive_change(
    path: PathBuf,
    desired: &[u8],
    previous_manifest: Option<&AdapterManifest>,
    mode: u32,
) -> Result<PendingChange, DaemonError> {
    exclusive_change_with_kind(
        path,
        desired,
        previous_manifest,
        mode,
        ManagedFileKind::Exclusive,
    )
}

fn exclusive_change_with_kind(
    path: PathBuf,
    desired: &[u8],
    previous_manifest: Option<&AdapterManifest>,
    mode: u32,
    kind: ManagedFileKind,
) -> Result<PendingChange, DaemonError> {
    let expected = capture_file_snapshot(&path)?;
    if let Some(current) = expected.content.as_deref() {
        let current_hash = sha256(current);
        let previously_managed = previous_manifest
            .and_then(|manifest| {
                manifest
                    .managed_files
                    .iter()
                    .find(|file| Path::new(&file.path) == path)
            })
            .is_some_and(|file| file.installed_hash == current_hash);
        if current != desired && !previously_managed {
            return Err(state_error(
                "project_agent_adapter_conflict",
                &format!(
                    "{} already exists and is not managed by this Clumsies integration.",
                    path.display()
                ),
            ));
        }
    }
    Ok(PendingChange {
        path,
        expected,
        desired: Some(desired.to_vec()),
        kind,
        mode,
    })
}

fn merged_change(
    path: PathBuf,
    kind: ManagedFileKind,
    mode: u32,
    render: impl FnOnce(Option<&[u8]>) -> Result<Vec<u8>, DaemonError>,
) -> Result<PendingChange, DaemonError> {
    let expected = capture_file_snapshot(&path)?;
    let desired = render(expected.content.as_deref())?;
    Ok(PendingChange {
        path,
        expected,
        desired: Some(desired),
        kind,
        mode,
    })
}

fn manifest_manages_path(manifest: Option<&AdapterManifest>, path: &Path) -> bool {
    manifest.is_some_and(|manifest| {
        manifest
            .managed_files
            .iter()
            .any(|file| Path::new(&file.path) == path)
    })
}

fn legacy_hook_is_proven_managed(
    manifest: Option<&AdapterManifest>,
    path: &Path,
    known_hash: &str,
) -> Result<bool, DaemonError> {
    let Some(content) = capture_file_snapshot(path)?.content else {
        return Ok(false);
    };
    let current_hash = sha256(&content);
    if current_hash == known_hash {
        return Ok(true);
    }
    Ok(manifest.is_some_and(|manifest| {
        manifest
            .managed_files
            .iter()
            .any(|file| Path::new(&file.path) == path && file.installed_hash == current_hash)
    }))
}

const BASE_AGENT_RUN_HOOKS: [(&str, u64); 4] = [
    ("UserPromptSubmit", 5),
    ("SubagentStart", 5),
    ("SubagentStop", 5),
    ("SessionEnd", 3),
];

// Stop is intentionally absent from fresh installs, but remains in the cleanup
// set so an update or remove can retire an exact handler owned by an older
// adapter without touching another tool's Stop hook.
const REMOVABLE_AGENT_RUN_HOOKS: [(&str, u64); 5] = [
    ("UserPromptSubmit", 5),
    ("Stop", 5),
    ("SubagentStart", 5),
    ("SubagentStop", 5),
    ("SessionEnd", 3),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct HookOwnership {
    lifecycle: bool,
    legacy_prompt: bool,
}

fn render_managed_hook_script(template: &str, runtime_binary: &str) -> String {
    let injected = format!("# Agent runtime: {}\n", shell_single_quote(runtime_binary));
    match template.find('\n') {
        Some(index) => {
            let mut rendered = String::with_capacity(template.len() + injected.len());
            rendered.push_str(&template[..=index]);
            rendered.push_str(&injected);
            rendered.push_str(&template[index + 1..]);
            rendered
        }
        None => format!("{template}\n{injected}"),
    }
}

fn render_managed_binary_resolver(
    adapter: ProjectAgentAdapterKind,
    runtime_binary: &str,
) -> String {
    let project_directory = match adapter {
        ProjectAgentAdapterKind::Codex => {
            r#"PROJECT_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)""#
        }
        ProjectAgentAdapterKind::ClaudeCode => r#"PROJECT_DIR="${CLAUDE_PROJECT_DIR:-$PWD}""#,
        ProjectAgentAdapterKind::Antigravity => r#"PROJECT_DIR="${ANTIGRAVITY_PROJECT_DIR:-$PWD}""#,
        ProjectAgentAdapterKind::Opencode | ProjectAgentAdapterKind::Dsh => {
            unreachable!("opencode and dsh do not install a resolver")
        }
    };
    let exported_project_directory = match adapter {
        ProjectAgentAdapterKind::Codex => "PROJECT_ROOT",
        ProjectAgentAdapterKind::ClaudeCode | ProjectAgentAdapterKind::Antigravity => "PROJECT_DIR",
        ProjectAgentAdapterKind::Opencode | ProjectAgentAdapterKind::Dsh => {
            unreachable!("opencode and dsh do not install a resolver")
        }
    };
    format!(
        "#!/usr/bin/env bash\n# Resolve only the App-bundled clumsiesd selected by Clumsies.\n\
         set -euo pipefail\n\n{project_directory}\n\n\
         CLUMSIES={}\n\
         [ -x \"$CLUMSIES\" ] || exit 0\n\n\
         export CLUMSIES {exported_project_directory}\n",
        shell_single_quote(runtime_binary)
    )
}

fn render_hook_registry(
    existing: Option<&[u8]>,
    script_path: &Path,
    include_stop_failure: bool,
    ownership: HookOwnership,
) -> Result<Vec<u8>, DaemonError> {
    let mut root = match existing {
        Some(content) => serde_json::from_slice::<Value>(content)
            .map_err(|_| adapter_conflict("The existing hook registry is not valid JSON."))?,
        None => Value::Object(Map::new()),
    };
    let root_object = root
        .as_object_mut()
        .ok_or_else(|| adapter_conflict("The hook registry must be a JSON object."))?;
    let hooks = root_object
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| adapter_conflict("The hook registry `hooks` value must be an object."))?;

    retire_hook_handler(hooks, "Stop", 5, script_path, ownership)?;
    for &(event, timeout) in &BASE_AGENT_RUN_HOOKS {
        install_hook_handler(hooks, event, timeout, script_path, ownership)?;
    }
    if include_stop_failure {
        install_hook_handler(hooks, "StopFailure", 5, script_path, ownership)?;
    }

    render_json(&root)
}

fn install_hook_handler(
    hooks: &mut Map<String, Value>,
    event: &str,
    timeout: u64,
    script_path: &Path,
    ownership: HookOwnership,
) -> Result<(), DaemonError> {
    let groups = hooks
        .entry(event)
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| {
            adapter_conflict(&format!(
                "The hook registry `{event}` value must be an array."
            ))
        })?;
    remove_owned_hook_handlers(groups, script_path, timeout, ownership)?;
    groups.push(json!({
        "hooks": [{
            "type": "command",
            "command": hook_command(script_path),
            "timeout": timeout
        }]
    }));
    Ok(())
}

fn retire_hook_handler(
    hooks: &mut Map<String, Value>,
    event: &str,
    timeout: u64,
    script_path: &Path,
    ownership: HookOwnership,
) -> Result<(), DaemonError> {
    let Some(value) = hooks.get_mut(event) else {
        return Ok(());
    };
    let groups = value.as_array_mut().ok_or_else(|| {
        adapter_conflict(&format!(
            "The hook registry `{event}` value must be an array."
        ))
    })?;
    remove_owned_hook_handlers(groups, script_path, timeout, ownership)?;
    if groups.is_empty() {
        hooks.remove(event);
    }
    Ok(())
}

fn remove_hook_registry(
    content: &[u8],
    script_path: &Path,
    include_stop_failure: bool,
) -> Result<Option<Vec<u8>>, DaemonError> {
    let mut root = serde_json::from_slice::<Value>(content)
        .map_err(|_| adapter_conflict("The existing hook registry is not valid JSON."))?;
    let root_object = root
        .as_object_mut()
        .ok_or_else(|| adapter_conflict("The hook registry must be a JSON object."))?;
    let Some(hooks) = root_object.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(Some(content.to_vec()));
    };

    let mut events = REMOVABLE_AGENT_RUN_HOOKS
        .iter()
        .map(|(event, _)| *event)
        .collect::<Vec<_>>();
    if include_stop_failure {
        events.push("StopFailure");
    }
    for event in events {
        let Some(groups) = hooks.get_mut(event).and_then(Value::as_array_mut) else {
            continue;
        };
        remove_owned_hook_handlers(
            groups,
            script_path,
            timeout_for_event(event),
            HookOwnership {
                lifecycle: true,
                legacy_prompt: false,
            },
        )?;
        if groups.is_empty() {
            hooks.remove(event);
        }
    }
    if hooks.is_empty() {
        root_object.remove("hooks");
    }
    if root_object.is_empty() {
        return Ok(None);
    }
    Ok(Some(render_json(&root)?))
}

fn timeout_for_event(event: &str) -> u64 {
    REMOVABLE_AGENT_RUN_HOOKS
        .iter()
        .find_map(|(candidate, timeout)| (*candidate == event).then_some(*timeout))
        .unwrap_or(5)
}

fn remove_owned_hook_handlers(
    groups: &mut Vec<Value>,
    script_path: &Path,
    lifecycle_timeout: u64,
    ownership: HookOwnership,
) -> Result<(), DaemonError> {
    let legacy_path = script_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("user-prompt-submit.sh");
    let mut lifecycle_exact = Vec::new();
    let mut lifecycle_drifted = false;
    let mut legacy_exact = Vec::new();
    let mut legacy_drifted = false;

    for (index, group) in groups.iter().enumerate() {
        if group_contains_local_hook_kind(group, script_path, LocalHookScriptKind::Lifecycle) {
            if is_exact_command_group(group, script_path, lifecycle_timeout) {
                lifecycle_exact.push(index);
            } else {
                lifecycle_drifted = true;
            }
        }
        if group_contains_local_hook_kind(group, script_path, LocalHookScriptKind::LegacyPrompt) {
            if is_exact_command_group(group, &legacy_path, 5) {
                legacy_exact.push(index);
            } else {
                legacy_drifted = true;
            }
        }
    }

    if (!lifecycle_exact.is_empty() || lifecycle_drifted) && !ownership.lifecycle {
        return Err(adapter_conflict(
            "The lifecycle hook path is already registered but is not owned by this Clumsies integration.",
        ));
    }
    if ownership.lifecycle && (lifecycle_drifted || lifecycle_exact.len() > 1) {
        return Err(adapter_conflict(
            "The managed lifecycle hook group changed after installation.",
        ));
    }
    if ownership.legacy_prompt && (legacy_drifted || legacy_exact.len() > 1) {
        return Err(adapter_conflict(
            "The managed legacy prompt hook group changed after installation.",
        ));
    }

    let mut indexes_to_remove = Vec::with_capacity(2);
    if ownership.lifecycle {
        indexes_to_remove.extend(lifecycle_exact);
    }
    if ownership.legacy_prompt {
        indexes_to_remove.extend(legacy_exact);
    }
    indexes_to_remove.sort_unstable_by(|left, right| right.cmp(left));
    indexes_to_remove.dedup();
    for index in indexes_to_remove {
        groups.remove(index);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocalHookScriptKind {
    Lifecycle,
    LegacyPrompt,
}

fn local_hook_script_kind(handler: &Value, script_path: &Path) -> Option<LocalHookScriptKind> {
    let object = handler.as_object()?;
    if object.get("type").and_then(Value::as_str) != Some("command") {
        return None;
    }
    let command = object.get("command").and_then(Value::as_str)?;
    let command_path = single_bash_script_path(command)?;
    let candidate = Path::new(&command_path);
    let expected_hooks_directory = script_path.parent()?;
    if candidate.parent()? != expected_hooks_directory {
        return None;
    }
    match candidate.file_name().and_then(|name| name.to_str()) {
        Some("agent-run-event.sh") => Some(LocalHookScriptKind::Lifecycle),
        Some("user-prompt-submit.sh") => Some(LocalHookScriptKind::LegacyPrompt),
        _ => None,
    }
}

fn group_contains_local_hook_kind(
    group: &Value,
    script_path: &Path,
    expected: LocalHookScriptKind,
) -> bool {
    group
        .as_object()
        .and_then(|object| object.get("hooks"))
        .and_then(Value::as_array)
        .is_some_and(|handlers| {
            handlers
                .iter()
                .any(|handler| local_hook_script_kind(handler, script_path) == Some(expected))
        })
}

fn is_exact_command_group(group: &Value, script_path: &Path, timeout: u64) -> bool {
    let Some(object) = group.as_object() else {
        return false;
    };
    if object.len() != 1 {
        return false;
    }
    let Some(handlers) = object.get("hooks").and_then(Value::as_array) else {
        return false;
    };
    handlers.len() == 1 && is_exact_command_handler(&handlers[0], script_path, timeout)
}

fn is_exact_command_handler(handler: &Value, script_path: &Path, timeout: u64) -> bool {
    let Some(object) = handler.as_object() else {
        return false;
    };
    if object.len() != 3
        || object.get("type").and_then(Value::as_str) != Some("command")
        || object.get("timeout").and_then(Value::as_u64) != Some(timeout)
    {
        return false;
    }
    let Some(command) = object.get("command").and_then(Value::as_str) else {
        return false;
    };
    single_bash_script_path(command).is_some_and(|path| Path::new(&path) == script_path)
}

fn single_bash_script_path(command: &str) -> Option<String> {
    let command = command.trim();
    let rest = command.strip_prefix("bash")?;
    if !rest.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut quote = Quote::None;
    let mut escaped = false;
    let mut path = String::new();
    let mut chars = rest.trim_start().chars();
    while let Some(character) = chars.next() {
        if escaped {
            path.push(character);
            escaped = false;
            continue;
        }
        match quote {
            Quote::Single => {
                if character == '\'' {
                    quote = Quote::None;
                } else {
                    path.push(character);
                }
            }
            Quote::Double => match character {
                '"' => quote = Quote::None,
                '\\' => escaped = true,
                '$' | '`' => return None,
                _ => path.push(character),
            },
            Quote::None => match character {
                '\'' => quote = Quote::Single,
                '"' => quote = Quote::Double,
                '\\' => escaped = true,
                character if character.is_whitespace() => {
                    if chars.all(|remaining| remaining.is_whitespace()) {
                        break;
                    }
                    return None;
                }
                '$' | '`' | ';' | '|' | '&' | '<' | '>' => return None,
                _ => path.push(character),
            },
        }
    }
    (quote == Quote::None && !escaped && !path.is_empty()).then_some(path)
}

fn hook_command(script_path: &Path) -> String {
    format!(
        "bash {}",
        shell_single_quote(&script_path.display().to_string())
    )
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn render_json(value: &Value) -> Result<Vec<u8>, DaemonError> {
    let mut rendered = serde_json::to_vec_pretty(value)?;
    rendered.push(b'\n');
    Ok(rendered)
}

fn render_antigravity_hooks(
    existing: Option<&[u8]>,
    script_path: &Path,
    ownership: HookOwnership,
) -> Result<Vec<u8>, DaemonError> {
    let mut root = match existing {
        Some(content) => serde_json::from_slice::<Value>(content).map_err(|_| {
            adapter_conflict("The existing Antigravity hook registry is not valid JSON.")
        })?,
        None => Value::Object(Map::new()),
    };
    let root_object = root
        .as_object_mut()
        .ok_or_else(|| adapter_conflict("The Antigravity hook registry must be a JSON object."))?;

    if let Some(existing_entry) = root_object.get("clumsies-agent-run-event") {
        if !ownership.lifecycle {
            return Err(adapter_conflict(
                "The Antigravity `clumsies-agent-run-event` hook entry is already registered but is not owned by this Clumsies integration.",
            ));
        }
        if !antigravity_hook_entry_matches(existing_entry, script_path) {
            return Err(adapter_conflict(
                "The managed Antigravity `clumsies-agent-run-event` hook entry changed after installation.",
            ));
        }
    }

    let hook_cmd = hook_command(script_path);
    root_object.insert(
        "clumsies-agent-run-event".to_owned(),
        json!({
            "PreInvocation": [{
                "type": "command",
                "command": hook_cmd,
                "timeout": 5
            }]
        }),
    );

    render_json(&root)
}

fn remove_antigravity_hooks(
    content: &[u8],
    script_path: &Path,
) -> Result<Option<Vec<u8>>, DaemonError> {
    let mut root = serde_json::from_slice::<Value>(content).map_err(|_| {
        adapter_conflict("The existing Antigravity hook registry is not valid JSON.")
    })?;
    let root_object = root
        .as_object_mut()
        .ok_or_else(|| adapter_conflict("The Antigravity hook registry must be a JSON object."))?;

    if let Some(entry) = root_object.get("clumsies-agent-run-event") {
        if !antigravity_hook_entry_matches(entry, script_path) {
            return Err(adapter_conflict(
                "The Antigravity `clumsies-agent-run-event` hook entry changed after installation.",
            ));
        }
        root_object.remove("clumsies-agent-run-event");
    }

    if root_object.is_empty() {
        return Ok(None);
    }
    Ok(Some(render_json(&root)?))
}

fn antigravity_hook_entry_matches(entry: &Value, script_path: &Path) -> bool {
    antigravity_current_hook_entry_matches(entry, script_path)
        || antigravity_legacy_hook_entry_matches(entry, script_path)
}

fn antigravity_current_hook_entry_matches(entry: &Value, script_path: &Path) -> bool {
    let Some(object) = entry.as_object() else {
        return false;
    };
    object.len() == 1 && antigravity_pre_invocation_matches(object, script_path)
}

fn antigravity_legacy_hook_entry_matches(entry: &Value, script_path: &Path) -> bool {
    let Some(object) = entry.as_object() else {
        return false;
    };
    let stop_matches = object
        .get("Stop")
        .and_then(Value::as_array)
        .is_some_and(|arr| arr.len() == 1 && is_exact_command_handler(&arr[0], script_path, 5));
    object.len() == 2 && antigravity_pre_invocation_matches(object, script_path) && stop_matches
}

fn antigravity_pre_invocation_matches(object: &Map<String, Value>, script_path: &Path) -> bool {
    object
        .get("PreInvocation")
        .and_then(Value::as_array)
        .is_some_and(|arr| arr.len() == 1 && is_exact_command_handler(&arr[0], script_path, 5))
}

fn render_codex_config(
    existing: Option<&[u8]>,
    runtime_binary: &str,
    previous_runtime_binary: Option<&str>,
) -> Result<Vec<u8>, DaemonError> {
    let mut document = match existing {
        Some(content) => std::str::from_utf8(content)
            .map_err(|_| adapter_conflict("The existing Codex config is not UTF-8."))?
            .parse::<DocumentMut>()
            .map_err(|_| adapter_conflict("The existing Codex config is not valid TOML."))?,
        None => DocumentMut::new(),
    };
    if !document.as_table().contains_key("mcp_servers") {
        document["mcp_servers"] = Item::Table(Table::new());
    }
    let servers = document["mcp_servers"]
        .as_table_mut()
        .ok_or_else(|| adapter_conflict("Codex `mcp_servers` must be a TOML table."))?;
    let mut args = Array::new();
    args.push("mcp");
    args.push("serve");
    if let Some(current) = servers.get_mut("clumsies") {
        let current = current.as_table_mut().ok_or_else(|| {
            adapter_conflict("Codex `mcp_servers.clumsies` must be a TOML table.")
        })?;
        let owned = previous_runtime_binary.is_some_and(|path| {
            codex_mcp_entry_matches(current, path)
                || codex_mcp_entry_matches(current, runtime_binary)
        });
        if !owned {
            return Err(adapter_conflict(
                "The Codex `mcp_servers.clumsies` entry is not managed by this Clumsies integration.",
            ));
        }
        // Only command and args are owned. Keep optional Codex fields such as
        // env/startup_timeout_sec and future user-added settings byte-for-byte.
        current.insert("command", value(runtime_binary));
        current.insert("args", value(args));
    } else {
        let mut clumsies = Table::new();
        clumsies.insert("command", value(runtime_binary));
        clumsies.insert("args", value(args));
        servers.insert("clumsies", Item::Table(clumsies));
    }
    Ok(document.to_string().into_bytes())
}

fn remove_codex_config(
    content: &[u8],
    runtime_binary: &str,
) -> Result<Option<Vec<u8>>, DaemonError> {
    let mut document = std::str::from_utf8(content)
        .map_err(|_| adapter_conflict("The existing Codex config is not UTF-8."))?
        .parse::<DocumentMut>()
        .map_err(|_| adapter_conflict("The existing Codex config is not valid TOML."))?;
    let Some(servers) = document
        .as_table_mut()
        .get_mut("mcp_servers")
        .and_then(Item::as_table_mut)
    else {
        return Ok(Some(content.to_vec()));
    };
    let Some(clumsies) = servers.get_mut("clumsies").and_then(Item::as_table_mut) else {
        return Ok(Some(content.to_vec()));
    };
    if !codex_mcp_entry_matches(clumsies, runtime_binary) {
        return Err(adapter_conflict(
            "The Codex `mcp_servers.clumsies` entry changed after installation.",
        ));
    }
    // Do not delete fields outside the managed command/args fragment.
    clumsies.remove("command");
    clumsies.remove("args");
    if clumsies.is_empty() {
        servers.remove("clumsies");
    }
    if servers.is_empty() {
        document.as_table_mut().remove("mcp_servers");
    }
    let rendered = document.to_string();
    Ok((!rendered.trim().is_empty()).then(|| rendered.into_bytes()))
}

fn codex_mcp_entry_matches(entry: &Table, runtime_binary: &str) -> bool {
    let command_matches = entry.get("command").and_then(Item::as_str) == Some(runtime_binary);
    let args_match = entry
        .get("args")
        .and_then(Item::as_array)
        .is_some_and(|args| {
            args.len() == 2
                && args.get(0).and_then(|item| item.as_str()) == Some("mcp")
                && args.get(1).and_then(|item| item.as_str()) == Some("serve")
        });
    command_matches && args_match
}

fn render_claude_mcp(
    existing: Option<&[u8]>,
    runtime_binary: &str,
    previous_runtime_binary: Option<&str>,
) -> Result<Vec<u8>, DaemonError> {
    let mut root = match existing {
        Some(content) => serde_json::from_slice::<Value>(content).map_err(|_| {
            adapter_conflict("The existing Claude Code MCP config is not valid JSON.")
        })?,
        None => Value::Object(Map::new()),
    };
    let root = root
        .as_object_mut()
        .ok_or_else(|| adapter_conflict("The Claude Code MCP config must be a JSON object."))?;
    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| adapter_conflict("Claude Code `mcpServers` must be a JSON object."))?;
    if let Some(current) = servers.get("clumsies") {
        let owned = previous_runtime_binary.is_some_and(|path| {
            current == &claude_mcp_entry(path) || current == &claude_mcp_entry(runtime_binary)
        });
        if !owned {
            return Err(adapter_conflict(
                "The Claude Code `mcpServers.clumsies` entry is not managed by this Clumsies integration.",
            ));
        }
    }
    servers.insert("clumsies".to_owned(), claude_mcp_entry(runtime_binary));
    let mut rendered = serde_json::to_vec_pretty(&Value::Object(root.clone()))?;
    rendered.push(b'\n');
    Ok(rendered)
}

fn claude_mcp_entry(runtime_binary: &str) -> Value {
    json!({
        "type": "stdio",
        "command": runtime_binary,
        "args": ["mcp", "serve"]
    })
}

/// opencode.json merge: register the clumsies MCP server under `mcp` and
/// append the clumsies plugin to the `plugin` array. User-owned keys and
/// other servers/plugins are preserved.
fn render_opencode_config(
    existing: Option<&[u8]>,
    runtime_binary: &str,
    previous_runtime_binary: Option<&str>,
) -> Result<Vec<u8>, DaemonError> {
    render_opencode_config_for_scope(existing, runtime_binary, previous_runtime_binary, true)
}

fn render_opencode_config_for_scope(
    existing: Option<&[u8]>,
    runtime_binary: &str,
    previous_runtime_binary: Option<&str>,
    repository: bool,
) -> Result<Vec<u8>, DaemonError> {
    let mut root = match existing {
        Some(content) => serde_json::from_slice::<Value>(content)
            .map_err(|_| adapter_conflict("The existing opencode config is not valid JSON."))?,
        None => Value::Object(Map::new()),
    };
    let root = root
        .as_object_mut()
        .ok_or_else(|| adapter_conflict("The opencode config must be a JSON object."))?;

    let mcp = root
        .entry("mcp")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| adapter_conflict("opencode `mcp` must be a JSON object."))?;
    if let Some(current) = mcp.get("clumsies") {
        let owned = previous_runtime_binary.is_some_and(|path| {
            current == &opencode_mcp_entry(path) || current == &opencode_mcp_entry(runtime_binary)
        });
        if !owned {
            return Err(adapter_conflict(
                "The opencode `mcp.clumsies` entry is not managed by this Clumsies integration.",
            ));
        }
    }
    mcp.insert("clumsies".to_owned(), opencode_mcp_entry(runtime_binary));

    if repository {
        const PLUGIN_SPEC: &str = "./.opencode/plugins/clumsies.ts";
        let plugins = root
            .entry("plugin")
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| adapter_conflict("opencode `plugin` must be an array."))?;
        if !plugins
            .iter()
            .any(|item| item.as_str() == Some(PLUGIN_SPEC))
        {
            plugins.push(Value::String(PLUGIN_SPEC.to_owned()));
        }
    }

    let mut rendered = serde_json::to_vec_pretty(&Value::Object(root.clone()))?;
    rendered.push(b'\n');
    Ok(rendered)
}

fn opencode_mcp_entry(runtime_binary: &str) -> Value {
    json!({
        "type": "local",
        "command": [runtime_binary, "mcp", "serve"],
        "enabled": true
    })
}

fn remove_opencode_config(
    content: &[u8],
    runtime_binary: &str,
) -> Result<Option<Vec<u8>>, DaemonError> {
    remove_opencode_config_for_scope(content, runtime_binary, true)
}

fn remove_opencode_config_for_scope(
    content: &[u8],
    runtime_binary: &str,
    repository: bool,
) -> Result<Option<Vec<u8>>, DaemonError> {
    let mut root = serde_json::from_slice::<Value>(content)
        .map_err(|_| adapter_conflict("The existing opencode config is not valid JSON."))?;
    let root = root
        .as_object_mut()
        .ok_or_else(|| adapter_conflict("The opencode config must be a JSON object."))?;

    if let Some(mcp) = root.get_mut("mcp").and_then(Value::as_object_mut) {
        if let Some(current) = mcp.get("clumsies") {
            let expected = opencode_mcp_entry(runtime_binary);
            if current != &expected {
                return Err(adapter_conflict(
                    "The opencode `mcp.clumsies` entry changed after installation.",
                ));
            }
            mcp.remove("clumsies");
        }
        if mcp.is_empty() {
            root.remove("mcp");
        }
    }

    if repository && let Some(plugins) = root.get_mut("plugin").and_then(Value::as_array_mut) {
        plugins.retain(|item| item.as_str() != Some("./.opencode/plugins/clumsies.ts"));
        if plugins.is_empty() {
            root.remove("plugin");
        }
    }

    if root.is_empty() {
        return Ok(None);
    }
    let mut rendered = serde_json::to_vec_pretty(&Value::Object(root.clone()))?;
    rendered.push(b'\n');
    Ok(Some(rendered))
}

/// Pins the App-bundled clumsiesd path into the plugin so Agent runtime
/// traffic cannot be redirected through PATH or a stale environment value.
fn render_opencode_plugin(runtime_binary: &str) -> String {
    let runtime_literal = serde_json::to_string(runtime_binary)
        .expect("serializing an Agent runtime path to a JSON string cannot fail");
    OPENCODE_PLUGIN.replace("\"__CLUMSIES_RUNTIME_BINARY__\"", &runtime_literal)
}

fn remove_claude_mcp(content: &[u8], runtime_binary: &str) -> Result<Option<Vec<u8>>, DaemonError> {
    let mut root = serde_json::from_slice::<Value>(content)
        .map_err(|_| adapter_conflict("The existing Claude Code MCP config is not valid JSON."))?;
    let object = root
        .as_object_mut()
        .ok_or_else(|| adapter_conflict("The Claude Code MCP config must be a JSON object."))?;
    let Some(servers) = object.get_mut("mcpServers").and_then(Value::as_object_mut) else {
        return Ok(Some(content.to_vec()));
    };
    let Some(current) = servers.get("clumsies") else {
        return Ok(Some(content.to_vec()));
    };
    let expected = claude_mcp_entry(runtime_binary);
    if current != &expected {
        return Err(adapter_conflict(
            "The Claude Code `mcpServers.clumsies` entry changed after installation.",
        ));
    }
    servers.remove("clumsies");
    if servers.is_empty() {
        object.remove("mcpServers");
    }
    if object.is_empty() {
        return Ok(None);
    }
    let mut rendered = serde_json::to_vec_pretty(&root)?;
    rendered.push(b'\n');
    Ok(Some(rendered))
}

fn manifest_for_changes(
    changes: &[PendingChange],
    runtime_binary: &Path,
    runtime_binary_hash: String,
) -> AdapterManifest {
    AdapterManifest {
        runtime_binary_hash,
        runtime_binary_path: runtime_binary.display().to_string(),
        delivery: ProjectAgentAdapterDelivery::LegacyFiles,
        managed_files: changes
            .iter()
            .filter_map(|change| {
                change.desired.as_ref().map(|content| ManagedFile {
                    path: change.path.display().to_string(),
                    kind: change.kind,
                    installed_hash: sha256(content),
                })
            })
            .collect(),
    }
}

impl ManagedPathGuard {
    fn capture_under(anchor: &Path, path: &Path) -> Result<Self, DaemonError> {
        validate_absolute_normal_path(anchor)?;
        validate_absolute_normal_path(path)?;
        let relative = path.strip_prefix(anchor).map_err(|_| {
            adapter_conflict(&format!(
                "Managed path {} escapes its adapter workspace {}.",
                path.display(),
                anchor.display()
            ))
        })?;
        if relative.as_os_str().is_empty() {
            return Err(adapter_conflict(
                "A managed adapter path cannot be its workspace root.",
            ));
        }

        let mut directories = Vec::new();
        capture_directory_identity(anchor, &mut directories)?;
        let mut current = anchor.to_path_buf();
        if let Some(parent) = relative.parent() {
            for component in parent.components() {
                let Component::Normal(component) = component else {
                    return Err(adapter_conflict(&format!(
                        "Managed path {} is not normalized.",
                        path.display()
                    )));
                };
                current.push(component);
                match fs::symlink_metadata(&current) {
                    Ok(metadata) => {
                        validate_directory_metadata(&current, &metadata)?;
                        directories.push(DirectoryIdentity::new(current.clone(), &metadata)?);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        validate_managed_leaf(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            anchor: anchor.to_path_buf(),
            directories,
        })
    }

    fn capture_inferred(path: &Path) -> Result<Self, DaemonError> {
        let anchor = inferred_managed_anchor(path)?;
        Self::capture_under(&anchor, path)
    }

    fn revalidate(&self) -> Result<(), DaemonError> {
        for identity in &self.directories {
            identity.revalidate()?;
        }

        let relative = self.path.strip_prefix(&self.anchor).map_err(|_| {
            adapter_conflict(&format!(
                "Managed path {} escaped its validated directory.",
                self.path.display()
            ))
        })?;
        let mut current = self.anchor.clone();
        if let Some(parent) = relative.parent() {
            for component in parent.components() {
                let Component::Normal(component) = component else {
                    return Err(adapter_conflict(&format!(
                        "Managed path {} is not normalized.",
                        self.path.display()
                    )));
                };
                current.push(component);
                match fs::symlink_metadata(&current) {
                    Ok(metadata) => validate_directory_metadata(&current, &metadata)?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        validate_managed_leaf(&self.path)
    }

    #[cfg(test)]
    fn create_parent_directories(&self) -> Result<(), DaemonError> {
        self.revalidate()?;
        let relative = self.path.strip_prefix(&self.anchor).map_err(|_| {
            adapter_conflict(&format!(
                "Managed path {} escaped its validated directory.",
                self.path.display()
            ))
        })?;
        let mut current = self.anchor.clone();
        if let Some(parent) = relative.parent() {
            for component in parent.components() {
                let Component::Normal(component) = component else {
                    return Err(adapter_conflict(&format!(
                        "Managed path {} is not normalized.",
                        self.path.display()
                    )));
                };
                current.push(component);
                match fs::symlink_metadata(&current) {
                    Ok(metadata) => validate_directory_metadata(&current, &metadata)?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        match fs::create_dir(&current) {
                            Ok(()) => {}
                            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                            Err(error) => return Err(error.into()),
                        }
                        let metadata = fs::symlink_metadata(&current)?;
                        validate_directory_metadata(&current, &metadata)?;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
        self.revalidate()
    }
}

impl DirectoryIdentity {
    fn new(path: PathBuf, metadata: &fs::Metadata) -> Result<Self, DaemonError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                path,
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            let canonical = fs::canonicalize(&path)?;
            Ok(Self { path, canonical })
        }
    }

    fn revalidate(&self) -> Result<(), DaemonError> {
        let metadata = fs::symlink_metadata(&self.path).map_err(|error| {
            adapter_conflict(&format!(
                "Managed directory {} changed while the adapter update was being applied: {error}",
                self.path.display()
            ))
        })?;
        validate_directory_metadata(&self.path, &metadata)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.dev() != self.device || metadata.ino() != self.inode {
                return Err(adapter_conflict(&format!(
                    "Managed directory {} was replaced while the adapter update was being applied.",
                    self.path.display()
                )));
            }
        }
        #[cfg(not(unix))]
        if fs::canonicalize(&self.path)? != self.canonical {
            return Err(adapter_conflict(&format!(
                "Managed directory {} was replaced while the adapter update was being applied.",
                self.path.display()
            )));
        }
        Ok(())
    }
}

fn validate_absolute_normal_path(path: &Path) -> Result<(), DaemonError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(adapter_conflict(&format!(
            "Managed path {} must be absolute and normalized.",
            path.display()
        )));
    }
    Ok(())
}

fn inferred_managed_anchor(path: &Path) -> Result<PathBuf, DaemonError> {
    validate_absolute_normal_path(path)?;
    for ancestor in path.ancestors().skip(1) {
        if matches!(
            ancestor.file_name().and_then(OsStr::to_str),
            Some(".config" | ".gemini")
        ) {
            return ancestor
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| adapter_conflict("The global harness directory has no home."));
        }
    }
    if matches!(
        path.file_name().and_then(OsStr::to_str),
        Some(".mcp.json" | ".claude.json" | "opencode.json")
    ) {
        return path.parent().map(Path::to_path_buf).ok_or_else(|| {
            adapter_conflict(&format!("Managed path {} has no parent.", path.display()))
        });
    }

    let mut ancestor = path.parent();
    while let Some(directory) = ancestor {
        if matches!(
            directory.file_name().and_then(OsStr::to_str),
            Some(".codex" | ".claude" | ".agents" | ".opencode" | ".clumsies" | ".dsh")
        ) {
            return directory.parent().map(Path::to_path_buf).ok_or_else(|| {
                adapter_conflict(&format!(
                    "Managed namespace {} has no containing directory.",
                    directory.display()
                ))
            });
        }
        ancestor = directory.parent();
    }
    path.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| adapter_conflict(&format!("Managed path {} has no parent.", path.display())))
}

fn capture_directory_identity(
    path: &Path,
    identities: &mut Vec<DirectoryIdentity>,
) -> Result<(), DaemonError> {
    let metadata = fs::symlink_metadata(path)?;
    validate_directory_metadata(path, &metadata)?;
    identities.push(DirectoryIdentity::new(path.to_path_buf(), &metadata)?);
    Ok(())
}

fn validate_directory_metadata(path: &Path, metadata: &fs::Metadata) -> Result<(), DaemonError> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(adapter_conflict(&format!(
            "Managed directory {} must be a real directory, not a symlink or another file type.",
            path.display()
        )));
    }
    Ok(())
}

fn validate_managed_leaf(path: &Path) -> Result<(), DaemonError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(adapter_conflict(&format!(
                "Managed file {} must be a regular file, not a symlink or another file type.",
                path.display()
            )))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
fn apply_changes(changes: &[PendingChange]) -> Result<Vec<FileBackup>, DaemonError> {
    // Validate the entire batch before the first mutation, so one malicious or
    // raced namespace cannot leave earlier paths partially updated.
    let guards = changes
        .iter()
        .map(|change| ManagedPathGuard::capture_inferred(&change.path))
        .collect::<Result<Vec<_>, _>>()?;
    for (change, guard) in changes.iter().zip(&guards) {
        guard.revalidate()?;
        require_file_snapshot(&change.path, &change.expected)?;
        guard.revalidate()?;
    }

    let mut backups = Vec::with_capacity(changes.len());
    for (change, guard) in changes.iter().zip(&guards) {
        guard.revalidate()?;
        require_file_snapshot(&change.path, &change.expected)?;
        guard.revalidate()?;
        let desired_mode = desired_file_mode(change);
        if change.expected.content == change.desired && change.expected.mode == desired_mode {
            continue;
        }
        let after = FileSnapshot {
            content: change.desired.clone(),
            mode: desired_mode,
            #[cfg(unix)]
            identity: None,
        };
        backups.push(FileBackup {
            path: change.path.clone(),
            before: change.expected.clone(),
            after,
        });
        let result = match &change.desired {
            // Shared files retain their existing permissions. This prevents a
            // 0600 user registry (notably ~/.claude.json) from becoming 0644.
            Some(content) => {
                atomic_write(&change.path, content, desired_mode.unwrap_or(change.mode))
            }
            None => remove_file_if_present(&change.path),
        };
        if let Err(error) = result {
            rollback_changes(&backups);
            return Err(error);
        }
    }
    Ok(backups)
}

#[cfg(test)]
fn rollback_changes(backups: &[FileBackup]) {
    for backup in backups.iter().rev() {
        let result = (|| {
            require_file_snapshot(&backup.path, &backup.after)?;
            match &backup.before.content {
                Some(content) => {
                    atomic_write(&backup.path, content, backup.before.mode.unwrap_or(0o644))
                }
                None => remove_file_if_present(&backup.path),
            }
        })();
        if let Err(error) = result {
            tracing::error!(
                "refused to roll back adapter file {} because it no longer matches this transaction: {error}",
                backup.path.display()
            );
        }
    }
}

#[cfg(test)]
fn require_file_snapshot(path: &Path, expected: &FileSnapshot) -> Result<(), DaemonError> {
    let current = capture_file_snapshot(path)?;
    if !snapshots_match(&current, expected) {
        return Err(state_error(
            "project_agent_adapter_conflict",
            &format!(
                "{} changed while the Coding Agent integration update was being applied.",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn capture_file_snapshot(path: &Path) -> Result<FileSnapshot, DaemonError> {
    let guard = ManagedPathGuard::capture_inferred(path)?;
    #[cfg(unix)]
    let snapshot = match ManagedLeafDirectory::open_existing(path)? {
        Some(directory) => file_snapshot_at(&directory, &directory.target)?,
        None => FileSnapshot {
            content: None,
            mode: None,
            identity: None,
        },
    };
    #[cfg(not(unix))]
    let snapshot = {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        FileSnapshot {
            content: read_optional(path)?,
            mode: file_mode(path)?,
        }
    };
    guard.revalidate()?;
    Ok(snapshot)
}

fn change_is_needed(change: &PendingChange) -> Result<bool, DaemonError> {
    let current = capture_file_snapshot(&change.path)?;
    if !snapshots_match(&current, &change.expected) {
        return Err(state_error(
            "project_agent_adapter_conflict",
            &format!(
                "{} changed after the Coding Agent integration update was planned.",
                change.path.display()
            ),
        ));
    }
    Ok(current.content != change.desired || current.mode != desired_file_mode(change))
}

fn desired_file_mode(change: &PendingChange) -> Option<u32> {
    change.desired.as_ref().map(|_| match change.kind {
        // Shared registries may contain credentials or other private member
        // configuration, so never broaden their existing permissions.
        ManagedFileKind::Exclusive => change.mode,
        _ => change.expected.mode.unwrap_or(change.mode),
    })
}

fn snapshots_match(current: &FileSnapshot, expected: &FileSnapshot) -> bool {
    current.content == expected.content && current.mode == expected.mode && {
        #[cfg(unix)]
        {
            expected.identity.is_none() || current.identity == expected.identity
        }
        #[cfg(not(unix))]
        {
            true
        }
    }
}

#[cfg(test)]
fn atomic_write(path: &Path, content: &[u8], mode: u32) -> Result<(), DaemonError> {
    let guard = ManagedPathGuard::capture_inferred(path)?;
    guard.create_parent_directories()?;
    let guard = ManagedPathGuard::capture_under(&guard.anchor, path)?;
    let parent = path.parent().ok_or_else(|| {
        DaemonError::InvalidRequest(format!("{} has no parent directory", path.display()))
    })?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let temporary = parent.join(format!(".{name}.clumsies-{}.tmp", Uuid::new_v4().simple()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(mode & 0o7777);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(content)?;
        file.sync_all()?;
        set_mode(&temporary, mode & 0o7777)?;
        guard.revalidate()?;
        fs::rename(&temporary, path)?;
        guard.revalidate()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
fn remove_file_if_present(path: &Path) -> Result<(), DaemonError> {
    let guard = ManagedPathGuard::capture_inferred(path)?;
    guard.revalidate()?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(target_os = "macos")]
fn verify_code_signature(path: &Path) -> Result<(), DaemonError> {
    let output = std::process::Command::new("/usr/bin/codesign")
        .arg("--verify")
        .arg("--strict")
        .arg(path)
        .output()?;
    if !output.status.success() {
        return Err(invalid_runtime_signature(&format!(
            "The bundled clumsiesd Agent runtime failed code-signature verification: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let runtime = code_signature_info(path)?;
    if runtime.identifier != DAEMON_AGENT_LABEL {
        return Err(invalid_runtime_signature(&format!(
            "The bundled Agent runtime has signing identifier {}, expected {DAEMON_AGENT_LABEL}.",
            runtime.identifier
        )));
    }

    let resident = std::env::current_exe()
        .ok()
        .and_then(|current| code_signature_info(&current).ok());
    if let Some(resident_team) = resident
        .as_ref()
        .and_then(|info| info.team_identifier.as_deref())
        && runtime.team_identifier.as_deref() != Some(resident_team)
    {
        return Err(invalid_runtime_signature(
            "The bundled Agent runtime is not signed by the resident daemon's team.",
        ));
    }

    // Local Debug apps are deliberately ad-hoc signed. Release packages must
    // carry both a real TeamIdentifier and the hardened-runtime flag, so an
    // arbitrary ad-hoc executable with the expected filename/identifier is
    // never accepted by the shipped daemon.
    if !cfg!(debug_assertions)
        && (runtime.ad_hoc || runtime.team_identifier.is_none() || !runtime.hardened_runtime)
    {
        return Err(invalid_runtime_signature(
            "The bundled Agent runtime does not have the required release signing identity.",
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
#[derive(Debug, PartialEq, Eq)]
struct CodeSignatureInfo {
    identifier: String,
    team_identifier: Option<String>,
    ad_hoc: bool,
    hardened_runtime: bool,
}

#[cfg(target_os = "macos")]
fn code_signature_info(path: &Path) -> Result<CodeSignatureInfo, DaemonError> {
    let output = std::process::Command::new("/usr/bin/codesign")
        .arg("--display")
        .arg("--verbose=4")
        .arg(path)
        .output()?;
    if !output.status.success() {
        return Err(invalid_runtime_signature(
            "The bundled Agent runtime signing identity could not be inspected.",
        ));
    }
    let details = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    parse_code_signature_info(&details).ok_or_else(|| {
        invalid_runtime_signature(
            "The bundled Agent runtime is missing required code-signature metadata.",
        )
    })
}

#[cfg(target_os = "macos")]
fn parse_code_signature_info(details: &str) -> Option<CodeSignatureInfo> {
    let identifier = details
        .lines()
        .find_map(|line| line.strip_prefix("Identifier="))?
        .trim()
        .to_owned();
    let team_identifier = details
        .lines()
        .find_map(|line| line.strip_prefix("TeamIdentifier="))
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "not set")
        .map(str::to_owned);
    let lower = details.to_ascii_lowercase();
    Some(CodeSignatureInfo {
        identifier,
        team_identifier,
        ad_hoc: lower.contains("signature=adhoc") || lower.contains("(adhoc"),
        hardened_runtime: lower
            .lines()
            .any(|line| line.contains("flags=") && line.contains("runtime")),
    })
}

#[cfg(target_os = "macos")]
fn invalid_runtime_signature(message: &str) -> DaemonError {
    state_error("project_agent_adapter_invalid_runtime", message)
}

fn canonical_agent_runtime_binary(path: &str) -> Result<PathBuf, DaemonError> {
    let path = path.trim();
    if path.is_empty() {
        return Err(DaemonError::InvalidRequest(
            "runtime_binary_path must identify the bundled clumsiesd Agent runtime".to_owned(),
        ));
    }
    let canonical = fs::canonicalize(path).map_err(|error| {
        DaemonError::InvalidRequest(format!(
            "Agent runtime path {path} cannot be resolved: {error}"
        ))
    })?;
    if !canonical.is_file() {
        return Err(DaemonError::InvalidRequest(format!(
            "Agent runtime path {} is not a file",
            canonical.display()
        )));
    }
    if !canonical.ends_with("Contents/Resources/clumsiesd") {
        return Err(DaemonError::InvalidRequest(format!(
            "Agent runtime path {} is not an App-bundled clumsiesd",
            canonical.display()
        )));
    }
    if !is_executable(&canonical)? {
        return Err(DaemonError::InvalidRequest(format!(
            "Agent runtime path {} is not executable",
            canonical.display()
        )));
    }
    Ok(canonical)
}

#[cfg(unix)]
fn is_executable(path: &Path) -> Result<bool, DaemonError> {
    use std::os::unix::fs::PermissionsExt;
    Ok(fs::metadata(path)?.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> Result<bool, DaemonError> {
    Ok(true)
}

#[cfg(not(unix))]
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, DaemonError> {
    match fs::read(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn sha256_file(path: &Path) -> Result<String, DaemonError> {
    Ok(sha256(&fs::read(path)?))
}

fn sha256(content: &[u8]) -> String {
    hex::encode(Sha256::digest(content))
}

#[cfg(all(test, unix))]
fn set_mode(path: &Path, mode: u32) -> Result<(), DaemonError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(all(test, not(unix)))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), DaemonError> {
    Ok(())
}

#[cfg(not(unix))]
fn file_mode(_path: &Path) -> Result<Option<u32>, DaemonError> {
    Ok(None)
}

#[cfg(all(test, unix))]
fn file_mode(path: &Path) -> Result<Option<u32>, DaemonError> {
    use std::os::unix::fs::PermissionsExt;
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata.permissions().mode() & 0o7777)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn cleanup_empty_adapter_directories(changes: &[PendingChange], workspace_root: &Path) {
    for change in changes.iter().rev() {
        let mut current = change.path.parent();
        while let Some(directory) = current {
            if directory == workspace_root {
                break;
            }
            if fs::remove_dir(directory).is_err() {
                break;
            }
            current = directory.parent();
        }
    }
}

fn adapter_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<DaemonProjectAgentAdapter, DaemonError> {
    Ok(adapter_record_from_row(row)?.status)
}

struct AdapterRecord {
    status: DaemonProjectAgentAdapter,
    manifest: AdapterManifest,
}

fn adapter_record_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<AdapterRecord, DaemonError> {
    let raw_adapter: String = row.try_get("adapter")?;
    let adapter = match raw_adapter.as_str() {
        "codex" => ProjectAgentAdapterKind::Codex,
        "claude-code" => ProjectAgentAdapterKind::ClaudeCode,
        "opencode" => ProjectAgentAdapterKind::Opencode,
        "dsh" => ProjectAgentAdapterKind::Dsh,
        "antigravity" => ProjectAgentAdapterKind::Antigravity,
        _ => {
            return Err(DaemonError::InvalidConfig(format!(
                "unknown persisted Coding Agent adapter {raw_adapter}"
            )));
        }
    };
    let manifest: AdapterManifest =
        serde_json::from_str(&row.try_get::<String, _>("manifest_json")?)?;
    Ok(AdapterRecord {
        status: DaemonProjectAgentAdapter {
            server_url: row.try_get("server_url")?,
            project_id: row.try_get("project_id")?,
            workspace_root: row.try_get("workspace_root")?,
            adapter,
            delivery: manifest.delivery,
            revision: row.try_get("revision")?,
            managed_files: manifest
                .managed_files
                .iter()
                .map(|file| file.path.clone())
                .collect(),
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        },
        manifest,
    })
}

fn required_value(name: &str, value: String) -> Result<String, DaemonError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(DaemonError::InvalidRequest(format!(
            "{name} must not be empty"
        )));
    }
    Ok(value.to_owned())
}

fn adapter_conflict(message: &str) -> DaemonError {
    state_error("project_agent_adapter_conflict", message)
}

fn state_error(code: &'static str, message: &str) -> DaemonError {
    DaemonError::State {
        code,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook_commands<'a>(registry: &'a Value, event: &str) -> Vec<&'a str> {
        registry["hooks"][event]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|group| group.get("hooks").and_then(Value::as_array))
            .flatten()
            .filter_map(|handler| handler.get("command").and_then(Value::as_str))
            .collect()
    }

    fn assert_hook_registry_migration(
        script: &Path,
        host_directory: &str,
        foreign_host_directory: &str,
        include_stop_failure: bool,
    ) {
        let current_command = hook_command(script);
        let current_legacy = format!(
            "bash \"{}\"",
            script
                .parent()
                .unwrap()
                .join("user-prompt-submit.sh")
                .display()
        );
        let foreign_lifecycle =
            format!("bash \"/old/workspace/{host_directory}/hooks/agent-run-event.sh\"");
        let foreign_legacy =
            format!("bash '/old/workspace/{host_directory}/hooks/user-prompt-submit.sh'");
        let foreign_known =
            format!("bash \"/old/workspace/{foreign_host_directory}/hooks/user-prompt-submit.sh\"");
        let foreign_same_host = format!("bash \"/opt/tools/{host_directory}/hooks/custom.sh\"");

        let mut hooks = Map::new();
        hooks.insert(
            "UserPromptSubmit".to_owned(),
            json!([
                {
                    "hooks": [
                        {"type": "command", "command": current_legacy, "timeout": 5}
                    ]
                },
                {
                    "hooks": [
                        {"type": "command", "command": foreign_legacy},
                        {"type": "command", "command": foreign_known},
                        {"type": "command", "command": foreign_same_host},
                        {"type": "command", "command": "echo foreign"}
                    ]
                }
            ]),
        );
        hooks.insert(
            "Stop".to_owned(),
            json!([
                {"hooks": [{"type": "command", "command": current_command, "timeout": 5}]},
                {"hooks": [{"type": "command", "command": foreign_lifecycle}]},
                {"hooks": [{"type": "command", "command": "echo stop foreign"}]}
            ]),
        );
        if include_stop_failure {
            hooks.insert(
                "StopFailure".to_owned(),
                json!([{
                    "hooks": [
                        {"type": "command", "command": foreign_lifecycle},
                        {"type": "command", "command": "echo failure foreign"}
                    ]
                }]),
            );
        }
        let existing = render_json(&json!({"theme": "dark", "hooks": hooks})).unwrap();

        let first = render_hook_registry(
            Some(&existing),
            script,
            include_stop_failure,
            HookOwnership {
                lifecycle: true,
                legacy_prompt: true,
            },
        )
        .unwrap();
        let second = render_hook_registry(
            Some(&first),
            script,
            include_stop_failure,
            HookOwnership {
                lifecycle: true,
                legacy_prompt: false,
            },
        )
        .unwrap();
        let rendered: Value = serde_json::from_slice(&second).unwrap();
        for &(event, _) in &BASE_AGENT_RUN_HOOKS {
            assert_eq!(
                hook_commands(&rendered, event)
                    .iter()
                    .filter(|command| **command == current_command)
                    .count(),
                1,
                "{event} should contain one current lifecycle handler"
            );
        }
        if include_stop_failure {
            assert_eq!(
                hook_commands(&rendered, "StopFailure")
                    .iter()
                    .filter(|command| **command == current_command)
                    .count(),
                1
            );
        } else {
            assert!(rendered["hooks"].get("StopFailure").is_none());
        }
        let prompt_commands = hook_commands(&rendered, "UserPromptSubmit");
        assert!(!prompt_commands.contains(&current_legacy.as_str()));
        assert!(prompt_commands.contains(&foreign_legacy.as_str()));
        assert!(prompt_commands.contains(&foreign_known.as_str()));
        assert!(prompt_commands.contains(&foreign_same_host.as_str()));
        assert!(prompt_commands.contains(&"echo foreign"));
        let stop_commands = hook_commands(&rendered, "Stop");
        assert!(!stop_commands.contains(&current_command.as_str()));
        assert!(stop_commands.contains(&foreign_lifecycle.as_str()));
        assert!(stop_commands.contains(&"echo stop foreign"));
        if include_stop_failure {
            let failure_commands = hook_commands(&rendered, "StopFailure");
            assert!(failure_commands.contains(&foreign_lifecycle.as_str()));
            assert!(failure_commands.contains(&"echo failure foreign"));
        }

        let removed = remove_hook_registry(&second, script, include_stop_failure)
            .unwrap()
            .unwrap();
        let removed: Value = serde_json::from_slice(&removed).unwrap();
        assert_eq!(removed["theme"], "dark");
        let prompt_commands = hook_commands(&removed, "UserPromptSubmit");
        assert!(!prompt_commands.contains(&current_legacy.as_str()));
        assert!(prompt_commands.contains(&foreign_legacy.as_str()));
        assert!(prompt_commands.contains(&foreign_known.as_str()));
        assert!(prompt_commands.contains(&foreign_same_host.as_str()));
        assert!(prompt_commands.contains(&"echo foreign"));
        let stop_commands = hook_commands(&removed, "Stop");
        assert!(!stop_commands.contains(&current_command.as_str()));
        assert!(stop_commands.contains(&foreign_lifecycle.as_str()));
        assert!(stop_commands.contains(&"echo stop foreign"));
        if include_stop_failure {
            assert_eq!(
                hook_commands(&removed, "StopFailure"),
                vec![foreign_lifecycle.as_str(), "echo failure foreign"]
            );
        }
    }

    #[test]
    fn codex_config_preserves_unrelated_tables() {
        let rendered = render_codex_config(
            Some(b"[model]\nname = \"gpt\"\n"),
            "/Applications/Clumsies.app/Contents/Resources/clumsiesd",
            None,
        )
        .unwrap();
        let text = String::from_utf8(rendered).unwrap();
        assert!(text.contains("[model]"));
        assert!(text.contains("name = \"gpt\""));
        assert!(text.contains("[mcp_servers.clumsies]"));
        assert!(
            text.contains("command = \"/Applications/Clumsies.app/Contents/Resources/clumsiesd\"")
        );
    }

    #[test]
    fn codex_config_preserves_foreign_fields_inside_the_clumsies_server() {
        let previous_runtime = "/Applications/Old Clumsies.app/Contents/Resources/clumsiesd";
        let current_runtime = "/Applications/Clumsies.app/Contents/Resources/clumsiesd";
        let existing = format!(
            "[mcp_servers.clumsies]\ncommand = {previous_runtime:?}\nargs = [\"mcp\", \"serve\"]\nstartup_timeout_sec = 45\nenabled = false\n\n[mcp_servers.clumsies.env]\nUSER_SETTING = \"kept\"\n"
        );

        let updated = render_codex_config(
            Some(existing.as_bytes()),
            current_runtime,
            Some(previous_runtime),
        )
        .unwrap();
        let updated_document = std::str::from_utf8(&updated)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        let entry = updated_document["mcp_servers"]["clumsies"]
            .as_table()
            .unwrap();
        assert_eq!(entry["command"].as_str(), Some(current_runtime));
        assert_eq!(entry["startup_timeout_sec"].as_integer(), Some(45));
        assert_eq!(entry["enabled"].as_bool(), Some(false));
        assert_eq!(entry["env"]["USER_SETTING"].as_str(), Some("kept"));

        let removed = remove_codex_config(&updated, current_runtime)
            .unwrap()
            .unwrap();
        let removed_document = std::str::from_utf8(&removed)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        let entry = removed_document["mcp_servers"]["clumsies"]
            .as_table()
            .unwrap();
        assert!(!entry.contains_key("command"));
        assert!(!entry.contains_key("args"));
        assert_eq!(entry["startup_timeout_sec"].as_integer(), Some(45));
        assert_eq!(entry["enabled"].as_bool(), Some(false));
        assert_eq!(entry["env"]["USER_SETTING"].as_str(), Some("kept"));
    }

    #[test]
    fn codex_manifest_declares_only_the_managed_server_fragments() {
        let runtime = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");
        let changes = vec![PendingChange {
            path: PathBuf::from("/tmp/workspace/.codex/config.toml"),
            expected: FileSnapshot {
                content: None,
                mode: None,
                #[cfg(unix)]
                identity: None,
            },
            desired: Some(
                b"[mcp_servers.clumsies]\ncommand = \"runtime\"\nargs = [\"mcp\", \"serve\"]\nenv = { USER_SETTING = \"foreign\" }\n"
                    .to_vec(),
            ),
            kind: ManagedFileKind::CodexConfig,
            mode: 0o644,
        }];
        let manifest = manifest_for_changes(&changes, runtime, "runtime-hash".to_owned());
        let serialized = serde_json::to_value(&manifest).unwrap();
        assert_eq!(
            serialized["managed_files"][0]["owned_fragments"],
            json!(["mcp_servers.clumsies.command", "mcp_servers.clumsies.args"])
        );
        assert!(
            !serialized["managed_files"][0]["owned_fragments"]
                .as_array()
                .unwrap()
                .iter()
                .any(|fragment| fragment.as_str().unwrap().contains("env"))
        );
        assert_eq!(
            serde_json::from_value::<AdapterManifest>(serialized).unwrap(),
            manifest
        );
    }

    #[test]
    fn hook_manifests_omit_stop_and_accept_only_the_exact_legacy_fragment_sets() {
        let cases: [(ManagedFileKind, &str, &[&str], &[&str]); 3] = [
            (
                ManagedFileKind::CodexHooks,
                "codex_hooks",
                &[
                    "hooks.UserPromptSubmit[clumsies-agent-run-event]",
                    "hooks.StopFailure[clumsies-agent-run-event]",
                ],
                &[
                    "hooks.UserPromptSubmit[clumsies-agent-run-event]",
                    "hooks.Stop[clumsies-agent-run-event]",
                    "hooks.StopFailure[clumsies-agent-run-event]",
                ],
            ),
            (
                ManagedFileKind::ClaudeSettings,
                "claude_settings",
                &[
                    "hooks.UserPromptSubmit[clumsies-agent-run-event]",
                    "hooks.StopFailure[clumsies-agent-run-event]",
                ],
                &[
                    "hooks.UserPromptSubmit[clumsies-agent-run-event]",
                    "hooks.Stop[clumsies-agent-run-event]",
                    "hooks.StopFailure[clumsies-agent-run-event]",
                ],
            ),
            (
                ManagedFileKind::AntigravityHooks,
                "antigravity_hooks",
                &["clumsies-agent-run-event.PreInvocation"],
                &[
                    "clumsies-agent-run-event.PreInvocation",
                    "clumsies-agent-run-event.Stop",
                ],
            ),
        ];

        for (kind, wire_kind, expected, legacy) in cases {
            let managed = ManagedFile {
                path: "/tmp/workspace/hooks.json".to_owned(),
                kind,
                installed_hash: "installed-hash".to_owned(),
            };
            let serialized = serde_json::to_value(&managed).unwrap();
            assert_eq!(serialized["owned_fragments"], json!(expected));
            assert!(
                serialized["owned_fragments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|fragment| !fragment.as_str().unwrap().ends_with(".Stop")
                        && !fragment.as_str().unwrap().starts_with("hooks.Stop["))
            );
            assert_eq!(
                serde_json::from_value::<ManagedFile>(serialized).unwrap(),
                managed
            );

            let legacy_manifest = json!({
                "path": "/tmp/workspace/hooks.json",
                "kind": wire_kind,
                "installed_hash": "installed-hash",
                "owned_fragments": legacy,
            });
            assert_eq!(
                serde_json::from_value::<ManagedFile>(legacy_manifest.clone()).unwrap(),
                managed
            );

            let mut forged = legacy_manifest;
            forged["owned_fragments"]
                .as_array_mut()
                .unwrap()
                .push(Value::String("foreign.fragment".to_owned()));
            assert!(serde_json::from_value::<ManagedFile>(forged).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn managed_namespace_symlink_is_rejected_before_any_write() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let victim = tempfile::tempdir().unwrap();
        fs::write(victim.path().join("config.toml"), b"victim\n").unwrap();
        symlink(victim.path(), workspace.path().join(".codex")).unwrap();

        let error = install_plan(
            ProjectAgentAdapterKind::Codex,
            workspace.path(),
            Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd"),
            None,
        )
        .unwrap_err();
        assert!(matches!(error, DaemonError::State { .. }));
        assert_eq!(
            fs::read(victim.path().join("config.toml")).unwrap(),
            b"victim\n"
        );
        assert!(!workspace.path().join(".agents").exists());
    }

    #[cfg(unix)]
    #[test]
    fn managed_namespace_identity_is_rechecked_before_batch_write() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let victim = tempfile::tempdir().unwrap();
        let changes = install_plan(
            ProjectAgentAdapterKind::Codex,
            workspace.path(),
            Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd"),
            None,
        )
        .unwrap();
        symlink(victim.path(), workspace.path().join(".codex")).unwrap();

        let error = apply_changes(&changes).unwrap_err();
        assert!(matches!(error, DaemonError::State { .. }));
        assert!(fs::read_dir(victim.path()).unwrap().next().is_none());
        assert!(!workspace.path().join(".agents").exists());
    }

    #[test]
    fn apply_rejects_a_shared_file_changed_after_planning_before_any_write() {
        let workspace = tempfile::tempdir().unwrap();
        let config_path = workspace.path().join("opencode.json");
        fs::write(&config_path, br#"{"model":"first"}"#).unwrap();
        let changes = install_plan(
            ProjectAgentAdapterKind::Opencode,
            workspace.path(),
            Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd"),
            None,
        )
        .unwrap();

        fs::write(&config_path, br#"{"model":"external"}"#).unwrap();
        let error = apply_changes(&changes).unwrap_err();
        assert!(matches!(
            error,
            DaemonError::State {
                code: "project_agent_adapter_conflict",
                ..
            }
        ));
        assert_eq!(fs::read(&config_path).unwrap(), br#"{"model":"external"}"#);
        assert!(
            !workspace
                .path()
                .join(".opencode/plugins/clumsies.ts")
                .exists()
        );
    }

    #[test]
    fn rollback_does_not_overwrite_a_file_changed_after_apply() {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join("opencode.json");
        fs::write(&path, b"before\n").unwrap();
        let expected = capture_file_snapshot(&path).unwrap();
        let changes = vec![PendingChange {
            path: path.clone(),
            expected,
            desired: Some(b"installed\n".to_vec()),
            kind: ManagedFileKind::OpencodeConfig,
            mode: 0o644,
        }];
        let backups = apply_changes(&changes).unwrap();
        fs::write(&path, b"external\n").unwrap();

        rollback_changes(&backups);

        assert_eq!(fs::read(path).unwrap(), b"external\n");
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_repairs_exclusive_modes_without_broadening_shared_files() {
        use std::os::unix::fs::PermissionsExt;

        let workspace = tempfile::tempdir().unwrap();
        let config = workspace.path().join(".codex/config.toml");
        let hook = workspace.path().join(".codex/hooks/agent-run-event.sh");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, b"[model]\nname = \"member\"\n").unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
        let runtime = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");
        let first = install_plan(
            ProjectAgentAdapterKind::Codex,
            workspace.path(),
            runtime,
            None,
        )
        .unwrap();
        apply_changes(&first).unwrap();
        assert_eq!(file_mode(&config).unwrap(), Some(0o600));

        let manifest = manifest_for_changes(&first, runtime, "runtime-hash".to_owned());
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o600)).unwrap();
        let reconcile = install_plan(
            ProjectAgentAdapterKind::Codex,
            workspace.path(),
            runtime,
            Some(&manifest),
        )
        .unwrap();
        assert!(
            reconcile
                .iter()
                .find(|change| change.path == hook)
                .is_some_and(|change| change_is_needed(change).unwrap())
        );
        apply_changes(&reconcile).unwrap();
        assert_eq!(file_mode(&hook).unwrap(), Some(0o755));
        assert_eq!(file_mode(&config).unwrap(), Some(0o600));
    }

    #[cfg(unix)]
    #[test]
    fn remove_rejects_a_replaced_managed_namespace_without_touching_victim() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let victim = tempfile::tempdir().unwrap();
        let runtime = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");
        let changes = install_plan(
            ProjectAgentAdapterKind::Codex,
            workspace.path(),
            runtime,
            None,
        )
        .unwrap();
        apply_changes(&changes).unwrap();
        let manifest = manifest_for_changes(&changes, runtime, "runtime-hash".to_owned());

        fs::remove_dir_all(workspace.path().join(".codex/hooks")).unwrap();
        fs::write(victim.path().join("keep"), b"untouched\n").unwrap();
        symlink(victim.path(), workspace.path().join(".codex/hooks")).unwrap();

        let error = remove_plan(&manifest, workspace.path()).unwrap_err();
        assert!(matches!(error, DaemonError::State { .. }));
        assert_eq!(
            fs::read(victim.path().join("keep")).unwrap(),
            b"untouched\n"
        );
        assert!(workspace.path().join(".codex/config.toml").exists());
    }

    #[cfg(unix)]
    #[test]
    fn shared_user_registry_keeps_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let workspace = tempfile::tempdir().unwrap();
        let registry = workspace.path().join(".claude.json");
        fs::write(
            &registry,
            br#"{"mcpServers":{"other":{"command":"other"}}}"#,
        )
        .unwrap();
        fs::set_permissions(&registry, fs::Permissions::from_mode(0o600)).unwrap();
        let changes = install_plan_with_claude_mcp_path(
            ProjectAgentAdapterKind::ClaudeCode,
            workspace.path(),
            Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd"),
            None,
            Some(&registry),
            None,
            None,
        )
        .unwrap();

        apply_changes(&changes).unwrap();
        assert_eq!(
            fs::metadata(&registry).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let value: Value = serde_json::from_slice(&fs::read(&registry).unwrap()).unwrap();
        assert_eq!(value["mcpServers"]["other"]["command"], "other");
        assert_eq!(
            value["mcpServers"]["clumsies"]["command"],
            "/Applications/Clumsies.app/Contents/Resources/clumsiesd"
        );

        let manifest = manifest_for_changes(
            &changes,
            Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd"),
            "runtime-hash".to_owned(),
        );
        let removals = remove_plan(&manifest, workspace.path()).unwrap();
        apply_changes(&removals).unwrap();
        assert_eq!(
            fs::metadata(&registry).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let value: Value = serde_json::from_slice(&fs::read(&registry).unwrap()).unwrap();
        assert_eq!(value["mcpServers"]["other"]["command"], "other");
        assert!(value["mcpServers"].get("clumsies").is_none());
    }

    #[test]
    fn claude_mcp_preserves_unrelated_servers() {
        let rendered = render_claude_mcp(
            Some(br#"{"mcpServers":{"other":{"command":"other"}}}"#),
            "/Applications/Clumsies.app/Contents/Resources/clumsiesd",
            None,
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&rendered).unwrap();
        assert_eq!(value["mcpServers"]["other"]["command"], "other");
        assert_eq!(
            value["mcpServers"]["clumsies"]["command"],
            "/Applications/Clumsies.app/Contents/Resources/clumsiesd"
        );
    }

    #[test]
    fn adapters_migrate_the_owned_legacy_helper_entry_to_clumsiesd() {
        let legacy = "/Users/test/Library/Application Support/ai.clumsies/bin/clumsies";
        let runtime = "/Applications/Clumsies.app/Contents/Resources/clumsiesd";

        let old_codex = render_codex_config(None, legacy, None).unwrap();
        let new_codex = render_codex_config(Some(&old_codex), runtime, Some(legacy)).unwrap();
        let new_codex = String::from_utf8(new_codex).unwrap();
        assert!(new_codex.contains(runtime));
        assert!(!new_codex.contains(legacy));

        let old_claude = render_claude_mcp(None, legacy, None).unwrap();
        let new_claude = render_claude_mcp(Some(&old_claude), runtime, Some(legacy)).unwrap();
        let new_claude: Value = serde_json::from_slice(&new_claude).unwrap();
        assert_eq!(new_claude["mcpServers"]["clumsies"]["command"], runtime);

        let old_opencode = render_opencode_config(None, legacy, None).unwrap();
        let new_opencode =
            render_opencode_config(Some(&old_opencode), runtime, Some(legacy)).unwrap();
        let new_opencode: Value = serde_json::from_slice(&new_opencode).unwrap();
        assert_eq!(new_opencode["mcp"]["clumsies"]["command"][0], runtime);
    }

    #[test]
    fn adapters_do_not_overwrite_unowned_clumsies_entries() {
        let runtime = "/Applications/Clumsies.app/Contents/Resources/clumsiesd";
        let codex = b"[mcp_servers.clumsies]\ncommand = \"foreign\"\nargs = []\n";
        assert!(render_codex_config(Some(codex), runtime, None).is_err());

        let claude = br#"{"mcpServers":{"clumsies":{"command":"foreign"}}}"#;
        assert!(render_claude_mcp(Some(claude), runtime, None).is_err());

        let opencode = br#"{"mcp":{"clumsies":{"type":"remote","url":"https://x"}}}"#;
        assert!(render_opencode_config(Some(opencode), runtime, None).is_err());

        let exact_codex = render_codex_config(None, runtime, None).unwrap();
        let exact_claude = render_claude_mcp(None, runtime, None).unwrap();
        let exact_opencode = render_opencode_config(None, runtime, None).unwrap();
        assert!(render_codex_config(Some(&exact_codex), runtime, None).is_err());
        assert!(render_claude_mcp(Some(&exact_claude), runtime, None).is_err());
        assert!(render_opencode_config(Some(&exact_opencode), runtime, None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn agent_runtime_path_requires_an_executable_in_a_macos_app_bundle() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let runtime = root
            .path()
            .join("Clumsies.app/Contents/Resources/clumsiesd");
        fs::create_dir_all(runtime.parent().unwrap()).unwrap();
        fs::write(&runtime, b"runtime").unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            canonical_agent_runtime_binary(runtime.to_str().unwrap()).unwrap(),
            fs::canonicalize(&runtime).unwrap()
        );

        let legacy = root.path().join("bin/clumsies");
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, b"legacy").unwrap();
        fs::set_permissions(&legacy, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(canonical_agent_runtime_binary(legacy.to_str().unwrap()).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn agent_runtime_rejects_a_valid_signature_with_the_wrong_identifier() {
        let root = tempfile::tempdir().unwrap();
        let runtime = root
            .path()
            .join("Clumsies.app/Contents/Resources/clumsiesd");
        fs::create_dir_all(runtime.parent().unwrap()).unwrap();
        fs::copy("/usr/bin/true", &runtime).unwrap();
        let canonical = canonical_agent_runtime_binary(runtime.to_str().unwrap()).unwrap();

        let error = verify_code_signature(&canonical).unwrap_err();
        assert!(matches!(
            error,
            DaemonError::State {
                code: "project_agent_adapter_invalid_runtime",
                ..
            }
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn code_signature_metadata_parser_requires_exact_release_identity_fields() {
        let info = parse_code_signature_info(
            "Executable=/tmp/clumsiesd\nIdentifier=ai.clumsies.daemon\nFormat=Mach-O\nCodeDirectory v=20500 size=1 flags=0x10000(runtime)\nTeamIdentifier=TEAM123\n",
        )
        .unwrap();
        assert_eq!(info.identifier, "ai.clumsies.daemon");
        assert_eq!(info.team_identifier.as_deref(), Some("TEAM123"));
        assert!(!info.ad_hoc);
        assert!(info.hardened_runtime);
    }

    #[test]
    fn codex_hooks_retire_proven_legacy_without_touching_foreign_handlers() {
        let script = Path::new("/tmp/workspace/.codex/hooks/agent-run-event.sh");
        assert_hook_registry_migration(script, ".codex", ".claude", false);
    }

    #[test]
    fn claude_hooks_retire_proven_legacy_without_touching_foreign_handlers() {
        let script = Path::new("/tmp/workspace/.claude/hooks/agent-run-event.sh");
        assert_hook_registry_migration(script, ".claude", ".codex", true);
    }

    #[test]
    fn fresh_hook_install_rejects_an_unowned_local_stop_handler() {
        for (script, include_stop_failure) in [
            (
                Path::new("/tmp/workspace/.codex/hooks/agent-run-event.sh"),
                false,
            ),
            (
                Path::new("/tmp/workspace/.claude/hooks/agent-run-event.sh"),
                true,
            ),
        ] {
            let existing = render_json(&json!({
                "hooks": {
                    "Stop": [{
                        "hooks": [{
                            "type": "command",
                            "command": hook_command(script),
                            "timeout": 5
                        }]
                    }]
                }
            }))
            .unwrap();
            let error = render_hook_registry(
                Some(&existing),
                script,
                include_stop_failure,
                HookOwnership::default(),
            )
            .unwrap_err();
            assert!(matches!(
                error,
                DaemonError::State {
                    code: "project_agent_adapter_conflict",
                    ..
                }
            ));
        }
    }

    #[test]
    fn remove_hook_registry_still_retires_an_old_managed_stop_handler() {
        let script = Path::new("/tmp/workspace/.codex/hooks/agent-run-event.sh");
        let existing = render_json(&json!({
            "theme": "dark",
            "hooks": {
                "Stop": [
                    {
                        "hooks": [{
                            "type": "command",
                            "command": hook_command(script),
                            "timeout": 5
                        }]
                    },
                    {
                        "hooks": [{
                            "type": "command",
                            "command": "echo foreign",
                            "timeout": 5
                        }]
                    }
                ]
            }
        }))
        .unwrap();

        let removed = remove_hook_registry(&existing, script, false)
            .unwrap()
            .unwrap();
        let removed: Value = serde_json::from_slice(&removed).unwrap();
        assert_eq!(removed["theme"], "dark");
        assert_eq!(hook_commands(&removed, "Stop"), vec!["echo foreign"]);
    }

    #[test]
    fn managed_hook_and_resolver_pin_the_bundled_daemon_runtime() {
        let runtime = "/Applications/Clumsies App.app/Contents/Resources/clumsiesd";
        let rendered =
            render_managed_hook_script("#!/usr/bin/env bash\nsource resolver.sh\n", runtime);
        assert!(rendered.starts_with(
            "#!/usr/bin/env bash\n# Agent runtime: '/Applications/Clumsies App.app/Contents/Resources/clumsiesd'\n"
        ));
        for adapter in [
            ProjectAgentAdapterKind::Codex,
            ProjectAgentAdapterKind::ClaudeCode,
        ] {
            let resolver = render_managed_binary_resolver(adapter, runtime);
            assert!(resolver.contains(runtime));
            assert!(!resolver.contains("zig-out"));
            assert!(!resolver.contains("command -v clumsies"));
            assert!(!resolver.contains(".clumsies/bin/clumsies"));
        }
    }

    #[test]
    fn install_plan_includes_lifecycle_assets_for_both_hosts() {
        let workspace = tempfile::tempdir().unwrap();
        let helper = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");

        let codex = install_plan(
            ProjectAgentAdapterKind::Codex,
            workspace.path(),
            helper,
            None,
        )
        .unwrap();
        assert!(codex.iter().any(|change| {
            change.path.ends_with(".codex/hooks.json") && change.kind == ManagedFileKind::CodexHooks
        }));
        assert!(codex.iter().any(|change| {
            change.path.ends_with(".codex/hooks/agent-run-event.sh") && change.mode == 0o755
        }));
        let codex_registry: Value = serde_json::from_slice(
            codex
                .iter()
                .find(|change| change.path.ends_with(".codex/hooks.json"))
                .and_then(|change| change.desired.as_deref())
                .unwrap(),
        )
        .unwrap();
        assert!(codex_registry["hooks"].get("Stop").is_none());
        assert!(codex_registry["hooks"].get("StopFailure").is_none());

        let claude = install_plan(
            ProjectAgentAdapterKind::ClaudeCode,
            workspace.path(),
            helper,
            None,
        )
        .unwrap();
        assert!(claude.iter().any(|change| {
            change.path.ends_with(".claude/settings.json")
                && change.kind == ManagedFileKind::ClaudeSettings
        }));
        assert!(claude.iter().any(|change| {
            change.path.ends_with(".claude/hooks/agent-run-event.sh") && change.mode == 0o755
        }));
        let claude_registry: Value = serde_json::from_slice(
            claude
                .iter()
                .find(|change| change.path.ends_with(".claude/settings.json"))
                .and_then(|change| change.desired.as_deref())
                .unwrap(),
        )
        .unwrap();
        assert!(claude_registry["hooks"].get("Stop").is_none());
        assert_eq!(hook_commands(&claude_registry, "StopFailure").len(), 1);
    }

    #[test]
    fn lifecycle_migration_does_not_claim_or_delete_unowned_legacy_scripts() {
        let helper = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");
        for (adapter, legacy_relative_path, registry_relative_path) in [
            (
                ProjectAgentAdapterKind::Codex,
                ".codex/hooks/user-prompt-submit.sh",
                ".codex/hooks.json",
            ),
            (
                ProjectAgentAdapterKind::ClaudeCode,
                ".claude/hooks/user-prompt-submit.sh",
                ".claude/settings.json",
            ),
        ] {
            let workspace = tempfile::tempdir().unwrap();
            let legacy_path = workspace.path().join(legacy_relative_path);
            let registry_path = workspace.path().join(registry_relative_path);
            fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
            fs::write(&legacy_path, b"#!/bin/sh\necho user-owned\n").unwrap();
            let legacy_command = hook_command(&legacy_path);
            fs::write(
                &registry_path,
                render_json(&json!({
                    "hooks": {
                        "UserPromptSubmit": [{
                            "hooks": [{
                                "type": "command",
                                "command": legacy_command,
                                "timeout": 5
                            }]
                        }]
                    }
                }))
                .unwrap(),
            )
            .unwrap();

            let changes = install_plan(adapter, workspace.path(), helper, None).unwrap();
            assert!(changes.iter().all(|change| change.path != legacy_path));
            apply_changes(&changes).unwrap();
            assert_eq!(
                fs::read(&legacy_path).unwrap(),
                b"#!/bin/sh\necho user-owned\n"
            );
            let installed_registry: Value =
                serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
            assert!(
                hook_commands(&installed_registry, "UserPromptSubmit")
                    .contains(&legacy_command.as_str())
            );

            let manifest = manifest_for_changes(&changes, helper, "helper-hash".to_owned());
            let removals = remove_plan(&manifest, workspace.path()).unwrap();
            assert!(removals.iter().all(|change| change.path != legacy_path));
            apply_changes(&removals).unwrap();
            assert_eq!(
                fs::read(&legacy_path).unwrap(),
                b"#!/bin/sh\necho user-owned\n"
            );
            let removed_registry: Value =
                serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
            assert!(
                hook_commands(&removed_registry, "UserPromptSubmit")
                    .contains(&legacy_command.as_str())
            );
        }
    }

    #[test]
    fn managed_lifecycle_wrapper_drift_conflicts_without_mutating_the_group() {
        let script = Path::new("/tmp/workspace/.codex/hooks/agent-run-event.sh");
        let command = hook_command(script);
        for group in [
            json!({
                "matcher": "special",
                "hooks": [{"type": "command", "command": command, "timeout": 5}]
            }),
            json!({
                "hooks": [
                    {"type": "command", "command": command, "timeout": 5},
                    {"type": "command", "command": "echo user-owned"}
                ]
            }),
        ] {
            let original = group.clone();
            let mut groups = vec![group];
            let error = remove_owned_hook_handlers(
                &mut groups,
                script,
                5,
                HookOwnership {
                    lifecycle: true,
                    legacy_prompt: false,
                },
            )
            .unwrap_err();
            assert!(matches!(error, DaemonError::State { .. }));
            assert_eq!(groups, vec![original]);
        }
    }

    #[test]
    fn opencode_config_preserves_unrelated_keys_and_servers() {
        let rendered = render_opencode_config(
            Some(
                br#"{"model":"deepseek/deepseek-chat","mcp":{"other":{"type":"remote","url":"https://x"}}}"#,
            ),
            "/Applications/Clumsies.app/Contents/Resources/clumsiesd",
            None,
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&rendered).unwrap();
        assert_eq!(value["model"], "deepseek/deepseek-chat");
        assert_eq!(value["mcp"]["other"]["url"], "https://x");
        assert_eq!(value["mcp"]["clumsies"]["type"], "local");
        assert_eq!(
            value["mcp"]["clumsies"]["command"][0],
            "/Applications/Clumsies.app/Contents/Resources/clumsiesd"
        );
        assert_eq!(value["mcp"]["clumsies"]["command"][1], "mcp");
        assert_eq!(value["mcp"]["clumsies"]["enabled"], true);
        assert!(
            value["plugin"]
                .as_array()
                .unwrap()
                .contains(&Value::String("./.opencode/plugins/clumsies.ts".to_owned()))
        );
    }

    #[test]
    fn opencode_config_render_is_idempotent() {
        let first = render_opencode_config(None, "/tmp/clumsiesd", None).unwrap();
        let second =
            render_opencode_config(Some(&first), "/tmp/clumsiesd", Some("/tmp/clumsiesd")).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn opencode_config_remove_restores_user_content() {
        let rendered = render_opencode_config(
            Some(br#"{"model":"deepseek/deepseek-chat"}"#),
            "/tmp/clumsiesd",
            None,
        )
        .unwrap();
        let removed = remove_opencode_config(&rendered, "/tmp/clumsiesd")
            .unwrap()
            .unwrap();
        let value: Value = serde_json::from_slice(&removed).unwrap();
        assert_eq!(value["model"], "deepseek/deepseek-chat");
        assert!(value.get("mcp").is_none());
        assert!(value.get("plugin").is_none());
    }

    #[test]
    fn opencode_remove_rejects_drifted_clumsies_entry() {
        let rendered = render_opencode_config(None, "/tmp/clumsiesd", None).unwrap();
        let mut mutated: Value = serde_json::from_slice(&rendered).unwrap();
        mutated["mcp"]["clumsies"]["command"][0] = Value::String("/tmp/foreign".to_owned());
        let mutated = render_json(&mutated).unwrap();
        let error = remove_opencode_config(&mutated, "/tmp/clumsiesd").unwrap_err();
        assert!(matches!(error, DaemonError::State { .. }));
    }

    #[test]
    fn opencode_plugin_pins_the_bundled_daemon_runtime() {
        let rendered =
            render_opencode_plugin("/Applications/Clumsies App.app/Contents/Resources/clumsiesd");
        assert!(
            rendered
                .contains("return \"/Applications/Clumsies App.app/Contents/Resources/clumsiesd\"")
        );
        assert!(!rendered.contains("process.env.CLUMSIES_BINARY"));
        assert!(rendered.contains(r#"hook_event_name: "StopFailure""#));
        assert!(!rendered.contains(r#"hook_event_name: "Stop""#));
        assert!(!rendered.contains(r#"hasError ? "StopFailure" : "Stop""#));
    }

    #[test]
    fn install_plan_includes_opencode_assets() {
        let workspace = tempfile::tempdir().unwrap();
        let helper = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");

        let changes = install_plan(
            ProjectAgentAdapterKind::Opencode,
            workspace.path(),
            helper,
            None,
        )
        .unwrap();
        assert!(changes.iter().any(|change| {
            change.path.ends_with("opencode.json") && change.kind == ManagedFileKind::OpencodeConfig
        }));
        assert!(changes.iter().any(|change| {
            change.path.ends_with(".opencode/plugins/clumsies.ts")
                && change.kind == ManagedFileKind::Exclusive
        }));
    }

    #[test]
    fn install_plan_includes_dsh_marker() {
        let workspace = tempfile::tempdir().unwrap();
        let helper = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");

        let changes = install_plan_with_project(
            ProjectAgentAdapterKind::Dsh,
            workspace.path(),
            helper,
            None,
            Some("https://clumsies.example.com"),
            Some("prj_dsh_test"),
        )
        .unwrap();
        assert_eq!(changes.len(), 1);
        let marker = &changes[0];
        assert_eq!(marker.path, workspace.path().join(".dsh/clumsies.json"));
        assert_eq!(marker.kind, ManagedFileKind::DshConfig);
        assert_eq!(marker.mode, 0o644);
        let rendered: Value = serde_json::from_slice(marker.desired.as_deref().unwrap()).unwrap();
        assert_eq!(rendered["server_url"], "https://clumsies.example.com");
        assert_eq!(rendered["project_id"], "prj_dsh_test");
        assert_eq!(
            rendered["runtime"],
            "/Applications/Clumsies.app/Contents/Resources/clumsiesd"
        );
    }

    #[test]
    fn dsh_plan_requires_project_context() {
        let workspace = tempfile::tempdir().unwrap();
        let helper = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");
        let error = install_plan_with_project(
            ProjectAgentAdapterKind::Dsh,
            workspace.path(),
            helper,
            None,
            None,
            None,
        )
        .unwrap_err();
        assert!(matches!(error, DaemonError::State { .. }));
    }

    #[test]
    fn dsh_install_remove_round_trip() {
        let workspace = tempfile::tempdir().unwrap();
        let helper = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");

        let changes = install_plan_with_project(
            ProjectAgentAdapterKind::Dsh,
            workspace.path(),
            helper,
            None,
            Some("https://clumsies.example.com"),
            Some("prj_dsh_test"),
        )
        .unwrap();
        apply_changes(&changes).unwrap();

        let marker_path = workspace.path().join(".dsh/clumsies.json");
        let marker: Value = serde_json::from_slice(&fs::read(&marker_path).unwrap()).unwrap();
        assert_eq!(marker["server_url"], "https://clumsies.example.com");
        assert_eq!(marker["project_id"], "prj_dsh_test");
        assert_eq!(
            marker["runtime"],
            "/Applications/Clumsies.app/Contents/Resources/clumsiesd"
        );

        let manifest = manifest_for_changes(&changes, helper, "helper-hash".to_owned());
        let removals = remove_plan(&manifest, workspace.path()).unwrap();
        apply_changes(&removals).unwrap();
        assert!(!marker_path.exists());
        cleanup_empty_adapter_directories(&removals, workspace.path());
        assert!(!workspace.path().join(".dsh").exists());
    }

    #[test]
    fn install_plan_includes_antigravity_artifacts() {
        let workspace = tempfile::tempdir().unwrap();
        let helper = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");

        let changes = install_plan(
            ProjectAgentAdapterKind::Antigravity,
            workspace.path(),
            helper,
            None,
        )
        .unwrap();

        assert!(changes.iter().any(|change| {
            change.path == workspace.path().join(".mcp.json")
                && change.kind == ManagedFileKind::ClaudeMcp
                && change.mode == 0o644
        }));
        assert!(changes.iter().any(|change| {
            change.path == workspace.path().join(".agents/hooks.json")
                && change.kind == ManagedFileKind::AntigravityHooks
                && change.mode == 0o644
        }));
        assert!(changes.iter().any(|change| {
            change.path == workspace.path().join(".agents/hooks/resolve-binary.sh")
                && change.kind == ManagedFileKind::Exclusive
                && change.mode == 0o755
        }));
        assert!(changes.iter().any(|change| {
            change.path == workspace.path().join(".agents/hooks/agent-run-event.sh")
                && change.kind == ManagedFileKind::Exclusive
                && change.mode == 0o755
        }));
        // Verify NO skills are installed
        assert!(
            changes
                .iter()
                .all(|change| !change.path.to_string_lossy().contains("skills"))
        );
    }

    #[test]
    fn codex_and_claude_install_plans_include_no_skills() {
        let workspace = tempfile::tempdir().unwrap();
        let helper = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");

        for adapter in [
            ProjectAgentAdapterKind::Codex,
            ProjectAgentAdapterKind::ClaudeCode,
        ] {
            let changes = install_plan(adapter, workspace.path(), helper, None).unwrap();
            assert!(
                changes
                    .iter()
                    .all(|change| !change.path.to_string_lossy().contains("skills")),
                "{adapter:?} install plan must not contain thin skills"
            );
        }
    }

    #[test]
    fn update_retires_stale_managed_skill_files() {
        let workspace = tempfile::tempdir().unwrap();
        let helper = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");

        // Simulate the pre-retirement state: the old manifest managed the
        // thin skill files and they exist on disk.
        let codex_activate = workspace.path().join(".agents/skills/activate/SKILL.md");
        let codex_ntmd = workspace.path().join(".agents/skills/ntmd/SKILL.md");
        let claude_activate = workspace.path().join(".claude/skills/activate/SKILL.md");
        let claude_ntmd = workspace.path().join(".claude/skills/ntmd/SKILL.md");
        for path in [&codex_activate, &codex_ntmd, &claude_activate, &claude_ntmd] {
            atomic_write(
                path,
                b"---
name: legacy
---
",
                0o644,
            )
            .unwrap();
        }
        let previous_manifest = AdapterManifest {
            runtime_binary_hash: "helper-hash".to_owned(),
            runtime_binary_path: helper.display().to_string(),
            delivery: ProjectAgentAdapterDelivery::LegacyFiles,
            managed_files: vec![
                ManagedFile {
                    path: codex_activate.display().to_string(),
                    kind: ManagedFileKind::Exclusive,
                    installed_hash: sha256(
                        b"---
name: legacy
---
",
                    ),
                },
                ManagedFile {
                    path: codex_ntmd.display().to_string(),
                    kind: ManagedFileKind::Exclusive,
                    installed_hash: sha256(
                        b"---
name: legacy
---
",
                    ),
                },
                ManagedFile {
                    path: claude_activate.display().to_string(),
                    kind: ManagedFileKind::Exclusive,
                    installed_hash: sha256(
                        b"---
name: legacy
---
",
                    ),
                },
                ManagedFile {
                    path: claude_ntmd.display().to_string(),
                    kind: ManagedFileKind::Exclusive,
                    installed_hash: sha256(
                        b"---
name: legacy
---
",
                    ),
                },
            ],
        };

        let changes = install_plan(
            ProjectAgentAdapterKind::Codex,
            workspace.path(),
            helper,
            Some(&previous_manifest),
        )
        .unwrap();
        let retired =
            retire_stale_managed_changes(&changes, Some(&previous_manifest), workspace.path())
                .unwrap();
        assert_eq!(retired.len(), 4);
        assert!(retired.iter().all(|change| change.desired.is_none()));

        let mut all_changes = changes.clone();
        all_changes.extend(retired);
        apply_changes(&all_changes).unwrap();
        cleanup_empty_adapter_directories(&all_changes, workspace.path());
        for path in [&codex_activate, &codex_ntmd, &claude_activate, &claude_ntmd] {
            assert!(!path.exists(), "{} must be retired", path.display());
        }
        assert!(!workspace.path().join(".agents").exists());

        let manifest = manifest_for_changes(&all_changes, helper, "helper-hash".to_owned());
        assert!(
            manifest
                .managed_files
                .iter()
                .all(|file| !file.path.contains("skills"))
        );
    }

    #[test]
    fn update_retire_conflicts_when_a_retired_file_changed() {
        let workspace = tempfile::tempdir().unwrap();
        let helper = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");

        let skill_path = workspace.path().join(".agents/skills/activate/SKILL.md");
        atomic_write(&skill_path, b"installed content", 0o644).unwrap();
        let previous_manifest = AdapterManifest {
            runtime_binary_hash: "helper-hash".to_owned(),
            runtime_binary_path: helper.display().to_string(),
            delivery: ProjectAgentAdapterDelivery::LegacyFiles,
            managed_files: vec![ManagedFile {
                path: skill_path.display().to_string(),
                kind: ManagedFileKind::Exclusive,
                installed_hash: sha256(b"installed content"),
            }],
        };
        // The user edited the file after install: retirement must conflict.
        atomic_write(&skill_path, b"user edited", 0o644).unwrap();
        let changes = install_plan(
            ProjectAgentAdapterKind::Codex,
            workspace.path(),
            helper,
            Some(&previous_manifest),
        )
        .unwrap();
        let error =
            retire_stale_managed_changes(&changes, Some(&previous_manifest), workspace.path())
                .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("changed after Clumsies installed it")
        );
    }

    #[test]
    fn antigravity_install_remove_round_trip() {
        let workspace = tempfile::tempdir().unwrap();
        let helper = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");

        let changes = install_plan(
            ProjectAgentAdapterKind::Antigravity,
            workspace.path(),
            helper,
            None,
        )
        .unwrap();
        apply_changes(&changes).unwrap();

        let mcp_path = workspace.path().join(".mcp.json");
        let hooks_path = workspace.path().join(".agents/hooks.json");
        let resolver_path = workspace.path().join(".agents/hooks/resolve-binary.sh");
        let hook_path = workspace.path().join(".agents/hooks/agent-run-event.sh");

        assert!(mcp_path.exists());
        assert!(hooks_path.exists());
        assert!(resolver_path.exists());
        assert!(hook_path.exists());

        let mcp_json: Value = serde_json::from_slice(&fs::read(&mcp_path).unwrap()).unwrap();
        assert_eq!(
            mcp_json["mcpServers"]["clumsies"]["command"],
            "/Applications/Clumsies.app/Contents/Resources/clumsiesd"
        );

        let hooks_json: Value = serde_json::from_slice(&fs::read(&hooks_path).unwrap()).unwrap();
        assert!(hooks_json["clumsies-agent-run-event"]["PreInvocation"].is_array());
        assert!(hooks_json["clumsies-agent-run-event"].get("Stop").is_none());

        let manifest = manifest_for_changes(&changes, helper, "helper-hash".to_owned());
        let removals = remove_plan(&manifest, workspace.path()).unwrap();
        apply_changes(&removals).unwrap();

        assert!(!mcp_path.exists());
        assert!(!hooks_path.exists());
        assert!(!resolver_path.exists());
        assert!(!hook_path.exists());
        cleanup_empty_adapter_directories(&removals, workspace.path());
        assert!(!workspace.path().join(".agents").exists());
    }

    #[test]
    fn antigravity_update_retires_legacy_stop_and_preserves_foreign_entries() {
        let script = Path::new("/tmp/workspace/.agents/hooks/agent-run-event.sh");
        let command = hook_command(script);
        let legacy = render_json(&json!({
            "lint-checker": {
                "PostToolUse": [{"type": "command", "command": "./lint.sh"}]
            },
            "clumsies-agent-run-event": {
                "PreInvocation": [{
                    "type": "command",
                    "command": command,
                    "timeout": 5
                }],
                "Stop": [{
                    "type": "command",
                    "command": command,
                    "timeout": 5
                }]
            }
        }))
        .unwrap();

        let migrated = render_antigravity_hooks(
            Some(&legacy),
            script,
            HookOwnership {
                lifecycle: true,
                legacy_prompt: false,
            },
        )
        .unwrap();
        let migrated_value: Value = serde_json::from_slice(&migrated).unwrap();
        assert!(migrated_value.get("lint-checker").is_some());
        assert!(migrated_value["clumsies-agent-run-event"]["PreInvocation"].is_array());
        assert!(
            migrated_value["clumsies-agent-run-event"]
                .get("Stop")
                .is_none()
        );
        assert_eq!(
            render_antigravity_hooks(
                Some(&migrated),
                script,
                HookOwnership {
                    lifecycle: true,
                    legacy_prompt: false,
                },
            )
            .unwrap(),
            migrated
        );

        let removed = remove_antigravity_hooks(&migrated, script)
            .unwrap()
            .unwrap();
        let removed: Value = serde_json::from_slice(&removed).unwrap();
        assert!(removed.get("lint-checker").is_some());
        assert!(removed.get("clumsies-agent-run-event").is_none());
    }

    #[test]
    fn antigravity_update_rejects_drifted_or_foreign_named_entries() {
        let script = Path::new("/tmp/workspace/.agents/hooks/agent-run-event.sh");
        let drifted = render_json(&json!({
            "clumsies-agent-run-event": {
                "PreInvocation": [{
                    "type": "command",
                    "command": hook_command(script),
                    "timeout": 9
                }],
                "Stop": [{
                    "type": "command",
                    "command": hook_command(script),
                    "timeout": 5
                }]
            }
        }))
        .unwrap();
        assert!(
            render_antigravity_hooks(
                Some(&drifted),
                script,
                HookOwnership {
                    lifecycle: true,
                    legacy_prompt: false,
                },
            )
            .is_err()
        );

        let unowned_legacy = render_json(&json!({
            "clumsies-agent-run-event": {
                "PreInvocation": [{
                    "type": "command",
                    "command": hook_command(script),
                    "timeout": 5
                }],
                "Stop": [{
                    "type": "command",
                    "command": hook_command(script),
                    "timeout": 5
                }]
            }
        }))
        .unwrap();
        assert!(
            render_antigravity_hooks(Some(&unowned_legacy), script, HookOwnership::default(),)
                .is_err()
        );

        let foreign = render_json(&json!({
            "clumsies-agent-run-event": {
                "PreInvocation": [{
                    "type": "command",
                    "command": "./foreign.sh",
                    "timeout": 5
                }]
            }
        }))
        .unwrap();
        assert!(
            render_antigravity_hooks(Some(&foreign), script, HookOwnership::default(),).is_err()
        );
    }

    #[test]
    fn antigravity_merges_existing_mcp_and_hooks() {
        let workspace = tempfile::tempdir().unwrap();
        let helper = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");

        let mcp_path = workspace.path().join(".mcp.json");
        fs::write(
            &mcp_path,
            br#"{"mcpServers":{"other":{"type":"stdio","command":"other-mcp"}}}"#,
        )
        .unwrap();

        let hooks_path = workspace.path().join(".agents/hooks.json");
        fs::create_dir_all(workspace.path().join(".agents")).unwrap();
        fs::write(
            &hooks_path,
            br#"{"lint-checker":{"PostToolUse":[{"matcher":"run_command","hooks":[{"type":"command","command":"./scripts/lint.sh","timeout":10}]}]}}"#,
        )
        .unwrap();

        let changes = install_plan(
            ProjectAgentAdapterKind::Antigravity,
            workspace.path(),
            helper,
            None,
        )
        .unwrap();
        apply_changes(&changes).unwrap();

        let merged_mcp: Value = serde_json::from_slice(&fs::read(&mcp_path).unwrap()).unwrap();
        assert_eq!(merged_mcp["mcpServers"]["other"]["command"], "other-mcp");
        assert_eq!(
            merged_mcp["mcpServers"]["clumsies"]["command"],
            "/Applications/Clumsies.app/Contents/Resources/clumsiesd"
        );

        let merged_hooks: Value = serde_json::from_slice(&fs::read(&hooks_path).unwrap()).unwrap();
        assert!(merged_hooks.get("lint-checker").is_some());
        assert!(merged_hooks.get("clumsies-agent-run-event").is_some());

        let manifest = manifest_for_changes(&changes, helper, "helper-hash".to_owned());
        let removals = remove_plan(&manifest, workspace.path()).unwrap();
        apply_changes(&removals).unwrap();

        let remaining_mcp: Value = serde_json::from_slice(&fs::read(&mcp_path).unwrap()).unwrap();
        assert_eq!(remaining_mcp["mcpServers"]["other"]["command"], "other-mcp");
        assert!(remaining_mcp["mcpServers"].get("clumsies").is_none());

        let remaining_hooks: Value =
            serde_json::from_slice(&fs::read(&hooks_path).unwrap()).unwrap();
        assert!(remaining_hooks.get("lint-checker").is_some());
        assert!(remaining_hooks.get("clumsies-agent-run-event").is_none());
    }

    #[test]
    fn opencode_install_remove_round_trip() {
        let workspace = tempfile::tempdir().unwrap();
        let helper = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");

        let changes = install_plan(
            ProjectAgentAdapterKind::Opencode,
            workspace.path(),
            helper,
            None,
        )
        .unwrap();
        apply_changes(&changes).unwrap();

        let config_path = workspace.path().join("opencode.json");
        let config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        assert_eq!(
            config["mcp"]["clumsies"]["command"][0],
            "/Applications/Clumsies.app/Contents/Resources/clumsiesd"
        );
        assert!(
            config["plugin"]
                .as_array()
                .unwrap()
                .contains(&Value::String("./.opencode/plugins/clumsies.ts".to_owned()))
        );
        assert!(
            workspace
                .path()
                .join(".opencode/plugins/clumsies.ts")
                .exists()
        );

        let manifest = manifest_for_changes(&changes, helper, "helper-hash".to_owned());
        let removals = remove_plan(&manifest, workspace.path()).unwrap();
        apply_changes(&removals).unwrap();
        assert!(!config_path.exists());
        assert!(
            !workspace
                .path()
                .join(".opencode/plugins/clumsies.ts")
                .exists()
        );
    }

    #[cfg(unix)]
    async fn journal_test_pool(workspace: &Path) -> (SqlitePool, String, String, String) {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE project_bindings (
                 server_url TEXT NOT NULL,
                 workspace_root TEXT NOT NULL,
                 project_id TEXT NOT NULL,
                 revision BIGINT NOT NULL,
                 created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                 updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                 PRIMARY KEY (server_url, workspace_root)
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        migrate(&pool).await.unwrap();
        let server_url = canonical_server_url("https://example.test/").unwrap();
        let workspace_root = fs::canonicalize(workspace).unwrap().display().to_string();
        let project_id = "project-journal".to_owned();
        sqlx::query(
            "INSERT INTO project_bindings
                 (server_url, workspace_root, project_id, revision)
             VALUES ($1, $2, $3, 1)",
        )
        .bind(&server_url)
        .bind(&workspace_root)
        .bind(&project_id)
        .execute(&pool)
        .await
        .unwrap();
        (pool, server_url, workspace_root, project_id)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn manifest_delivery_migration_rewrites_live_and_pending_records() {
        let workspace = tempfile::tempdir().unwrap();
        let (pool, server_url, workspace_root, project_id) =
            journal_test_pool(workspace.path()).await;
        let legacy = json!({
            "runtime_binary_hash": "a".repeat(64),
            "runtime_binary_path": "/Applications/Clumsies.app/Contents/Resources/clumsiesd",
            "managed_files": []
        })
        .to_string();
        assert!(serde_json::from_str::<AdapterManifest>(&legacy).is_err());
        sqlx::query(
            "INSERT INTO project_agent_adapters
                 (server_url, workspace_root, project_id, adapter, revision, manifest_json)
             VALUES ($1, $2, $3, 'codex', 1, $4)",
        )
        .bind(&server_url)
        .bind(&workspace_root)
        .bind(&project_id)
        .bind(&legacy)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO adapter_fs_ops
                 (operation_id, server_url, workspace_root, project_id, adapter, action,
                  expected_revision, next_revision, manifest_json, changes_json)
             VALUES ($1, $2, $3, $4, 'opencode', 'install', NULL, 1, $5, '[]')",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&server_url)
        .bind(&workspace_root)
        .bind(&project_id)
        .bind(&legacy)
        .execute(&pool)
        .await
        .unwrap();

        migrate(&pool).await.unwrap();
        migrate(&pool).await.unwrap();
        for table in ["project_agent_adapters", "adapter_fs_ops"] {
            let query = format!("SELECT manifest_json FROM {table} LIMIT 1");
            let raw: String = sqlx::query_scalar(&query).fetch_one(&pool).await.unwrap();
            let manifest: AdapterManifest = serde_json::from_str(&raw).unwrap();
            assert_eq!(manifest.delivery, ProjectAgentAdapterDelivery::LegacyFiles);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn codex_plugin_transition_recovers_cleanup_and_preserves_user_content() {
        let workspace = tempfile::tempdir().unwrap();
        let (pool, server_url, workspace_root, project_id) =
            journal_test_pool(workspace.path()).await;
        let workspace_root = PathBuf::from(workspace_root);
        let helper = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");
        let config_path = workspace_root.join(".codex/config.toml");
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(&config_path, "[model]\nname = \"gpt\"\n").unwrap();
        let hooks_path = workspace_root.join(".codex/hooks.json");
        fs::write(
            &hooks_path,
            br#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"lint","timeout":2}]}]}}"#,
        )
        .unwrap();

        let legacy_changes = install_plan(
            ProjectAgentAdapterKind::Codex,
            &workspace_root,
            helper,
            None,
        )
        .unwrap();
        apply_changes(&legacy_changes).unwrap();
        let legacy_manifest = manifest_for_changes(&legacy_changes, helper, "a".repeat(64));
        sqlx::query(
            "INSERT INTO project_agent_adapters
                 (server_url, workspace_root, project_id, adapter, revision, manifest_json)
             VALUES ($1, $2, $3, 'codex', 1, $4)",
        )
        .bind(&server_url)
        .bind(workspace_root.display().to_string())
        .bind(&project_id)
        .bind(serde_json::to_string(&legacy_manifest).unwrap())
        .execute(&pool)
        .await
        .unwrap();

        let cleanup = remove_plan(&legacy_manifest, &workspace_root).unwrap();
        let host_manifest = AdapterManifest {
            runtime_binary_hash: "a".repeat(64),
            runtime_binary_path: helper.display().to_string(),
            delivery: ProjectAgentAdapterDelivery::HostPlugin,
            managed_files: Vec::new(),
        };
        let operation = prepared_adapter_fs_op(
            PreparedAdapterFsOp {
                global: false,
                operation_id: Uuid::new_v4().to_string(),
                server_url,
                workspace_root: workspace_root.clone(),
                project_id,
                adapter: ProjectAgentAdapterKind::Codex,
                action: AdapterFsAction::Install,
                expected_revision: Some(1),
                next_revision: Some(2),
                manifest_json: Some(serde_json::to_string(&host_manifest).unwrap()),
                changes: Vec::new(),
            },
            &cleanup,
        )
        .unwrap();
        persist_prepared_adapter_fs_op(&pool, &operation)
            .await
            .unwrap();
        apply_journal_change_cas(&operation, 0, &operation.changes[0]).unwrap();
        recover_pending_fs_ops(&pool).await.unwrap();

        let config = fs::read_to_string(&config_path).unwrap();
        assert!(config.contains("[model]"));
        assert!(!config.contains("mcp_servers.clumsies"));
        let hooks: Value = serde_json::from_slice(&fs::read(&hooks_path).unwrap()).unwrap();
        assert!(hooks["hooks"].get("PreToolUse").is_some());
        assert!(hooks["hooks"].get("UserPromptSubmit").is_none());
        assert!(
            !workspace_root
                .join(".codex/hooks/agent-run-event.sh")
                .exists()
        );
        assert!(
            !workspace_root
                .join(".codex/hooks/resolve-binary.sh")
                .exists()
        );
        let raw: String = sqlx::query_scalar(
            "SELECT manifest_json FROM project_agent_adapters WHERE adapter = 'codex'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let manifest: AdapterManifest = serde_json::from_str(&raw).unwrap();
        assert_eq!(manifest.delivery, ProjectAgentAdapterDelivery::HostPlugin);
        assert!(manifest.managed_files.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn codex_plugin_cleanup_accepts_already_missing_legacy_shared_files() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_root = fs::canonicalize(workspace.path()).unwrap();
        let helper = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");
        let legacy_manifest = AdapterManifest {
            runtime_binary_hash: "a".repeat(64),
            runtime_binary_path: helper.display().to_string(),
            delivery: ProjectAgentAdapterDelivery::LegacyFiles,
            managed_files: vec![
                ManagedFile {
                    path: workspace_root
                        .join(".codex/config.toml")
                        .display()
                        .to_string(),
                    kind: ManagedFileKind::CodexConfig,
                    installed_hash: "b".repeat(64),
                },
                ManagedFile {
                    path: workspace_root
                        .join(".codex/hooks.json")
                        .display()
                        .to_string(),
                    kind: ManagedFileKind::CodexHooks,
                    installed_hash: "c".repeat(64),
                },
            ],
        };
        let cleanup = remove_plan(&legacy_manifest, &workspace_root).unwrap();
        assert!(
            cleanup
                .iter()
                .all(|change| { change.expected.content.is_none() && change.desired.is_none() })
        );

        let host_manifest = AdapterManifest {
            delivery: ProjectAgentAdapterDelivery::HostPlugin,
            managed_files: Vec::new(),
            ..legacy_manifest
        };
        prepared_adapter_fs_op(
            PreparedAdapterFsOp {
                global: false,
                operation_id: Uuid::new_v4().to_string(),
                server_url: "https://app.clumsies.ai".to_owned(),
                workspace_root,
                project_id: "prj_missing_legacy_files".to_owned(),
                adapter: ProjectAgentAdapterKind::Codex,
                action: AdapterFsAction::Install,
                expected_revision: Some(1),
                next_revision: Some(2),
                manifest_json: Some(serde_json::to_string(&host_manifest).unwrap()),
                changes: Vec::new(),
            },
            &cleanup,
        )
        .unwrap();
    }

    #[cfg(unix)]
    fn journal_test_install_operation(
        server_url: String,
        workspace_root: &Path,
        project_id: String,
    ) -> PreparedAdapterFsOp {
        let config_path = workspace_root.join("opencode.json");
        fs::write(&config_path, b"before-config\n").unwrap();
        set_mode(&config_path, 0o600).unwrap();
        let plugin_path = workspace_root.join(".opencode/plugins/clumsies.ts");
        let changes = vec![
            PendingChange {
                path: config_path,
                expected: FileSnapshot {
                    content: Some(b"before-config\n".to_vec()),
                    mode: Some(0o600),
                    identity: None,
                },
                desired: Some(b"after-config\n".to_vec()),
                kind: ManagedFileKind::OpencodeConfig,
                mode: 0o644,
            },
            PendingChange {
                path: plugin_path,
                expected: FileSnapshot {
                    content: None,
                    mode: None,
                    identity: None,
                },
                desired: Some(b"export const plugin = true;\n".to_vec()),
                kind: ManagedFileKind::Exclusive,
                mode: 0o644,
            },
        ];
        let runtime = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");
        let manifest = manifest_for_changes(&changes, runtime, "a".repeat(64));
        prepared_adapter_fs_op(
            PreparedAdapterFsOp {
                global: false,
                operation_id: Uuid::new_v4().to_string(),
                server_url,
                workspace_root: workspace_root.to_path_buf(),
                project_id,
                adapter: ProjectAgentAdapterKind::Opencode,
                action: AdapterFsAction::Install,
                expected_revision: None,
                next_revision: Some(1),
                manifest_json: Some(serde_json::to_string(&manifest).unwrap()),
                changes: Vec::new(),
            },
            &changes,
        )
        .unwrap()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn prepared_adapter_journal_recovers_a_mid_batch_process_crash() {
        let workspace = tempfile::tempdir().unwrap();
        let (pool, server_url, workspace_root, project_id) =
            journal_test_pool(workspace.path()).await;
        let operation =
            journal_test_install_operation(server_url, Path::new(&workspace_root), project_id);
        persist_prepared_adapter_fs_op(&pool, &operation)
            .await
            .unwrap();
        assert!(has_pending_fs_ops(&pool).await.unwrap());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA synchronous")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1,
            "the dedicated FULL journal barrier must restore normal pool operation"
        );

        apply_journal_change_cas(&operation, 0, &operation.changes[0]).unwrap();
        assert_eq!(
            fs::read(Path::new(&workspace_root).join("opencode.json")).unwrap(),
            b"after-config\n"
        );
        assert!(
            !Path::new(&workspace_root)
                .join(".opencode/plugins/clumsies.ts")
                .exists()
        );
        let plugin_parent = Path::new(&workspace_root).join(".opencode/plugins");
        fs::create_dir_all(&plugin_parent).unwrap();
        let crashed_stage = plugin_parent.join(
            operation.changes[1]
                .stage_name
                .as_deref()
                .expect("write journal has a persisted stage name"),
        );
        fs::write(&crashed_stage, b"partial-private-stage").unwrap();

        recover_pending_fs_ops(&pool).await.unwrap();
        assert!(!has_pending_fs_ops(&pool).await.unwrap());
        assert_eq!(
            fs::read(Path::new(&workspace_root).join("opencode.json")).unwrap(),
            b"after-config\n"
        );
        assert_eq!(
            fs::read(Path::new(&workspace_root).join(".opencode/plugins/clumsies.ts")).unwrap(),
            b"export const plugin = true;\n"
        );
        assert!(!crashed_stage.exists());
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT revision FROM project_agent_adapters
                 WHERE server_url = $1 AND workspace_root = $2 AND adapter = 'opencode'",
            )
            .bind("https://example.test")
            .bind(&workspace_root)
            .fetch_one(&pool)
            .await
            .unwrap(),
            1
        );
        recover_pending_fs_ops(&pool).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn prepared_adapter_journal_preserves_an_external_leaf_and_stays_pending() {
        let workspace = tempfile::tempdir().unwrap();
        let (pool, server_url, workspace_root, project_id) =
            journal_test_pool(workspace.path()).await;
        let operation =
            journal_test_install_operation(server_url, Path::new(&workspace_root), project_id);
        persist_prepared_adapter_fs_op(&pool, &operation)
            .await
            .unwrap();
        fs::write(
            Path::new(&workspace_root).join("opencode.json"),
            b"external-editor\n",
        )
        .unwrap();

        let error = recover_pending_fs_ops(&pool).await.unwrap_err();
        assert!(matches!(error, DaemonError::State { .. }));
        assert!(has_pending_fs_ops(&pool).await.unwrap());
        assert_eq!(
            fs::read(Path::new(&workspace_root).join("opencode.json")).unwrap(),
            b"external-editor\n"
        );
        assert!(
            !Path::new(&workspace_root)
                .join(".opencode/plugins/clumsies.ts")
                .exists()
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM project_agent_adapters")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn prepared_adapter_journal_recovers_a_mid_remove_process_crash() {
        let workspace = tempfile::tempdir().unwrap();
        let (pool, server_url, workspace_root, project_id) =
            journal_test_pool(workspace.path()).await;
        let install = journal_test_install_operation(
            server_url.clone(),
            Path::new(&workspace_root),
            project_id.clone(),
        );
        persist_prepared_adapter_fs_op(&pool, &install)
            .await
            .unwrap();
        apply_prepared_adapter_fs_op(&install).unwrap();
        finalize_prepared_adapter_fs_op(&pool, &install)
            .await
            .unwrap();

        let config_path = Path::new(&workspace_root).join("opencode.json");
        let plugin_path = Path::new(&workspace_root).join(".opencode/plugins/clumsies.ts");
        let changes = vec![
            PendingChange {
                path: config_path.clone(),
                expected: capture_file_snapshot(&config_path).unwrap(),
                desired: Some(b"before-config\n".to_vec()),
                kind: ManagedFileKind::OpencodeConfig,
                mode: 0o644,
            },
            PendingChange {
                path: plugin_path.clone(),
                expected: capture_file_snapshot(&plugin_path).unwrap(),
                desired: None,
                kind: ManagedFileKind::Exclusive,
                mode: 0o644,
            },
        ];
        let remove = prepared_adapter_fs_op(
            PreparedAdapterFsOp {
                global: false,
                operation_id: Uuid::new_v4().to_string(),
                server_url,
                workspace_root: PathBuf::from(&workspace_root),
                project_id,
                adapter: ProjectAgentAdapterKind::Opencode,
                action: AdapterFsAction::Remove,
                expected_revision: Some(1),
                next_revision: None,
                manifest_json: None,
                changes: Vec::new(),
            },
            &changes,
        )
        .unwrap();
        persist_prepared_adapter_fs_op(&pool, &remove)
            .await
            .unwrap();
        apply_journal_change_cas(&remove, 0, &remove.changes[0]).unwrap();

        recover_pending_fs_ops(&pool).await.unwrap();
        assert_eq!(fs::read(config_path).unwrap(), b"before-config\n");
        assert!(!plugin_path.exists());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM project_agent_adapters")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert!(!has_pending_fs_ops(&pool).await.unwrap());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn leaf_cas_never_replaces_a_file_created_after_capture() {
        let workspace = tempfile::tempdir().unwrap();
        let (pool, server_url, workspace_root, project_id) =
            journal_test_pool(workspace.path()).await;
        let operation =
            journal_test_install_operation(server_url, Path::new(&workspace_root), project_id);
        persist_prepared_adapter_fs_op(&pool, &operation)
            .await
            .unwrap();

        let change = &operation.changes[0];
        let path = Path::new(&workspace_root).join(&change.relative_path);
        let directory = ManagedLeafDirectory::open(&path).unwrap();
        let (old_name, _) = journal_transient_names(&operation, 0).unwrap();
        rename_noreplace_at(&directory, &directory.target, &old_name).unwrap();
        directory.sync().unwrap();
        fs::write(&path, b"external-after-capture\n").unwrap();

        let error = apply_journal_change_cas(&operation, 0, change).unwrap_err();
        assert!(matches!(error, DaemonError::State { .. }));
        assert_eq!(fs::read(&path).unwrap(), b"external-after-capture\n");
        assert_eq!(
            file_snapshot_at(&directory, &old_name)
                .unwrap()
                .content
                .unwrap(),
            b"before-config\n"
        );
        assert!(has_pending_fs_ops(&pool).await.unwrap());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn adapter_journal_rejects_untrusted_paths_and_stage_names() {
        let workspace = tempfile::tempdir().unwrap();
        let (_pool, server_url, workspace_root, project_id) =
            journal_test_pool(workspace.path()).await;
        let operation =
            journal_test_install_operation(server_url, Path::new(&workspace_root), project_id);

        let mut escaped = operation.clone();
        escaped.changes[0].relative_path = "../victim".to_owned();
        assert!(validate_prepared_adapter_fs_op(&escaped).is_err());

        let mut forged_stage = operation;
        forged_stage.changes[0].stage_name = Some("opencode.json".to_owned());
        assert!(validate_prepared_adapter_fs_op(&forged_stage).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn bounded_leaf_snapshot_rejects_a_fifo_without_blocking() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let workspace = tempfile::tempdir().unwrap();
        let fifo = workspace.path().join("opencode.json");
        let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        let started = std::time::Instant::now();
        assert!(capture_file_snapshot(&fifo).is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn bounded_leaf_snapshot_rejects_oversize_files() {
        let workspace = tempfile::tempdir().unwrap();
        let file = workspace.path().join("opencode.json");
        let handle = fs::File::create(&file).unwrap();
        handle
            .set_len((MAX_ADAPTER_FS_CONTENT_BYTES + 1) as u64)
            .unwrap();
        drop(handle);
        assert!(capture_file_snapshot(&file).is_err());
    }
}
