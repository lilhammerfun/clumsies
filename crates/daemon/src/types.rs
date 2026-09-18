//! Daemon IPC data contracts and validation.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;
use thiserror::Error;

use crate::CredentialStoreError;
use crate::server_client::is_retryable_http_status;
use crate::util::validate_draft_resource_path;

pub(crate) fn project_binding_from_row(
    row: &SqliteRow,
) -> Result<DaemonProjectBinding, DaemonError> {
    Ok(DaemonProjectBinding {
        server_url: row.try_get("server_url")?,
        workspace_root: row.try_get("workspace_root")?,
        project_id: row.try_get("project_id")?,
        revision: row.try_get("revision")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonIpcTransport {
    MacosXpcMachService,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonIpcEndpoint {
    pub transport: DaemonIpcTransport,
    pub service_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonIpcRequest {
    pub method: String,
    pub payload: serde_json::Value,
    /// Present only for short-lived Agent protocol proxies. Desktop and other
    /// resident clients omit this marker and retain the existing IPC contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_runtime: Option<AgentRuntimeIdentity>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub request_id: String,
}

impl DaemonIpcRequest {
    pub fn new(method: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            method: method.into(),
            payload,
            agent_runtime: None,
            request_id: crate::diagnostics::new_request_id(),
        }
    }

    pub fn empty(method: impl Into<String>) -> Self {
        Self::new(method, json!({}))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonIpcResponse {
    pub ok: bool,
    pub payload: serde_json::Value,
    pub error: Option<ApiError>,
}

impl DaemonIpcResponse {
    pub(crate) fn from_result(result: Result<serde_json::Value, DaemonError>) -> Self {
        match result {
            Ok(payload) => Self {
                ok: true,
                payload,
                error: None,
            },
            Err(error) => Self {
                ok: false,
                payload: json!({}),
                error: Some(api_error_from_daemon_error(error)),
            },
        }
    }

    pub fn into_payload<T>(self) -> Result<T, DaemonError>
    where
        T: DeserializeOwned,
    {
        if self.ok {
            serde_json::from_value(self.payload).map_err(DaemonError::from)
        } else {
            Err(match self.error {
                Some(error) => DaemonError::Remote(error),
                None => DaemonError::Ipc("daemon IPC call failed without error details".to_owned()),
            })
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonBootstrapStatus {
    pub label: String,
    pub mach_service_name: String,
    pub plist_path: String,
    pub installed: bool,
    pub endpoint: DaemonIpcEndpoint,
    pub runtime: LaunchAgentRuntimeStatus,
}

impl DaemonBootstrapStatus {
    pub(crate) fn with_runtime(mut self, runtime: LaunchAgentRuntimeStatus) -> Self {
        self.runtime = runtime;
        self
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LaunchAgentRuntimeStatus {
    pub installed: bool,
    pub bootstrapped: bool,
    pub running: bool,
    pub pid: Option<u32>,
    pub state: Option<String>,
    pub last_exit_code: Option<i32>,
    pub last_error: Option<String>,
}

impl LaunchAgentRuntimeStatus {
    pub fn from_launchctl_print(installed: bool, output: &str) -> Self {
        let mut status = Self {
            installed,
            bootstrapped: true,
            running: false,
            pid: None,
            state: None,
            last_exit_code: None,
            last_error: None,
        };
        for raw_line in output.lines() {
            let line = raw_line.trim();
            if let Some(value) = line.strip_prefix("pid =") {
                status.pid = value.trim().parse::<u32>().ok();
                continue;
            }
            if let Some(value) = line.strip_prefix("state =") {
                let value = value.trim();
                status.state = (!value.is_empty()).then(|| value.to_owned());
                continue;
            }
            if let Some(value) = line
                .strip_prefix("last exit code =")
                .or_else(|| line.strip_prefix("last exit status ="))
            {
                status.last_exit_code = value.trim().parse::<i32>().ok();
            }
        }
        status.running = status.pid.is_some()
            || status
                .state
                .as_deref()
                .is_some_and(|state| state == "running");
        status
    }

    pub(crate) fn not_bootstrapped(installed: bool, last_error: Option<String>) -> Self {
        Self {
            installed,
            bootstrapped: false,
            running: false,
            pid: None,
            state: None,
            last_exit_code: None,
            last_error,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectConfig {
    pub server_url: String,
    pub project_id: Option<String>,
    #[serde(default)]
    pub memory_guidelines_path: Option<String>,
    pub has_access_token: bool,
    pub has_refresh_token: bool,
    pub ready: bool,
    pub missing_fields: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectConfigUpdateRequest {
    pub server_url: String,
    pub project_id: Option<String>,
    #[serde(default)]
    pub memory_guidelines_path: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectSelectionRequest {
    pub project_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectBindingResolveRequest {
    pub workspace_path: String,
    #[serde(default)]
    pub required_adapter: Option<ProjectAgentAdapterRuntimeRequirement>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProjectAgentAdapterRuntimeRequirement {
    pub adapter: crate::ProjectAgentAdapterKind,
    pub delivery: crate::ProjectAgentAdapterDelivery,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectBindingListRequest {
    pub project_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectBindingListResponse {
    pub items: Vec<DaemonProjectBinding>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectBindingReplaceRequest {
    pub workspace_root: String,
    pub project_id: String,
    pub expected_revision: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectBindingRemoveRequest {
    pub workspace_root: String,
    pub expected_revision: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectBindingRemoveResponse {
    pub workspace_root: String,
    pub removed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectBinding {
    pub server_url: String,
    pub workspace_root: String,
    pub project_id: String,
    pub revision: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonServerRequest {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonServerResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectConfigReadiness {
    pub(crate) ready: bool,
    pub(crate) missing_fields: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonHealth {
    pub daemon_version: String,
    pub agent_runtime: AgentRuntimeIdentity,
    pub server_url: String,
    pub project_id: Option<String>,
    pub daemon_installation_id: String,
    pub log_dir: String,
    pub local_db: LocalDbStatus,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentRuntimeIdentity {
    pub protocol_revision: u32,
    pub build_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LocalDbStatus {
    pub path: String,
    pub ready: bool,
    pub schema_version: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonSyncStatus {
    pub draft_sync: SyncChannelStatus,
    pub commit_sync: SyncChannelStatus,
    pub pending_operation_count: i64,
    pub failed_operation_count: i64,
    pub behind_draft_count: i64,
    pub reconciliation_conflict_count: i64,
    pub last_success_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SyncChannelStatus {
    pub state: SyncState,
    pub server_cursor: Option<String>,
    pub last_attempt_at: Option<String>,
    pub last_success_at: Option<String>,
    pub last_error: Option<ApiError>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    Idle,
    Queued,
    Syncing,
    Retrying,
    Degraded,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonSyncRetryRequest {
    pub channel: SyncRetryChannel,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectSyncStatusRequest {
    pub project_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonProjectSyncRetryRequest {
    pub project_id: String,
    pub channel: SyncRetryChannel,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncRetryChannel {
    Drafts,
    Commits,
    All,
}

impl SyncRetryChannel {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Drafts => "drafts",
            Self::Commits => "commits",
            Self::All => "all",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonRetryResponse {
    pub retry_id: String,
    pub started: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Default, PartialEq, Eq)]
pub struct DaemonDraftListQuery {
    pub resource: Option<String>,
    pub status: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonDraftListResponse {
    pub items: Vec<DaemonDraftSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonDraftDetail {
    pub draft: DaemonDraftSummary,
    pub operations: Vec<DaemonLocalDraftOperation>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonDraftSummary {
    pub draft_id: String,
    pub project_id: String,
    pub server_draft_id: Option<String>,
    pub server_version: i64,
    pub base_commit_id: Option<String>,
    pub current_commit_id: Option<String>,
    pub freshness: DaemonDraftFreshness,
    pub has_upstream_resource_changes: bool,
    pub reconciliation: DaemonDraftReconciliationStatus,
    pub reconciliation_candidate_id: Option<String>,
    pub scope: DaemonDraftScope,
    pub resource_kind: DaemonDraftResourceKind,
    pub target_id: Option<String>,
    pub path: Option<String>,
    pub status: DaemonLocalDraftStatus,
    pub created_at: String,
    pub updated_at: String,
    pub pending_operation_count: i64,
    pub failed_operation_count: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonLocalDraftOperation {
    pub local_operation_id: String,
    pub resource_kind: DaemonDraftResourceKind,
    pub operation: DaemonDraftOperation,
    pub source: DaemonDraftOperationRecordSource,
    pub sync_status: DraftOperationSyncStatus,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonLocalDraftStatus {
    Open,
    Submitted,
    Merged,
    Discarded,
}

impl DaemonLocalDraftStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Submitted => "submitted",
            Self::Merged => "merged",
            Self::Discarded => "discarded",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonDraftFreshness {
    Current,
    Behind,
}

impl DaemonDraftFreshness {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Behind => "behind",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonDraftReconciliationStatus {
    Unknown,
    Clean,
    Conflicts,
}

impl DaemonDraftReconciliationStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Clean => "clean",
            Self::Conflicts => "conflicts",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonDraftOperationRequest {
    #[serde(default)]
    pub draft_id: Option<String>,
    #[serde(default)]
    pub base_commit_id: Option<String>,
    pub project_id: String,
    pub scope: DaemonDraftScope,
    pub resource: DaemonDraftResourceKind,
    pub op: DaemonDraftOperation,
    pub source: Option<DaemonDraftOperationSource>,
}

/// Desktop request to create one set of Organization Memory proposals atomically.
#[derive(Debug, Deserialize)]
pub(crate) struct DaemonCreateMemoryDraftsRequest {
    /// Project carrying the proposed documents before publication.
    pub project_id: String,
    /// Organization head against which the caller checked the document paths.
    pub base_commit_id: Option<String>,
    /// Create-only operations; duplicate or already drafted paths are rejected.
    pub operations: Vec<DaemonDraftOperation>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonDraftDetailRequest {
    pub draft_id: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonDraftScope {
    Org,
    Project,
}

impl DaemonDraftScope {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Org => "org",
            Self::Project => "project",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonDraftResourceKind {
    Memory,
}

impl DaemonDraftResourceKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonDraftOperationSource {
    Desktop,
    Cli,
    McpStore,
}

impl DaemonDraftOperationSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Cli => "cli",
            Self::McpStore => "mcp_store",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonDraftOperationRecordSource {
    Desktop,
    Cli,
    McpStore,
    Server,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonDraftOperation {
    pub create: Option<DaemonCreateDraftOperation>,
    pub update: Option<DaemonUpdateDraftOperation>,
    pub rename: Option<DaemonRenameDraftOperation>,
    pub delete: Option<DaemonDeleteDraftOperation>,
    pub discard: Option<DaemonDiscardDraftOperation>,
}

impl DaemonDraftOperation {
    pub(crate) fn validate(&self, resource: DaemonDraftResourceKind) -> Result<(), DaemonError> {
        let count = [
            self.create.is_some(),
            self.update.is_some(),
            self.rename.is_some(),
            self.delete.is_some(),
            self.discard.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count();
        if count != 1 {
            return Err(DaemonError::InvalidRequest(
                "draft operation must contain exactly one operation variant".to_owned(),
            ));
        }
        if let Some(create) = &self.create {
            validate_draft_resource_path(resource, &create.path)?;
        }
        if let Some(rename) = &self.rename {
            validate_draft_resource_path(resource, &rename.new_path)?;
        }
        if let Some(update) = &self.update {
            update.validate()?;
        }
        let content = self
            .create
            .as_ref()
            .map(|operation| &operation.content)
            .or_else(|| {
                self.update
                    .as_ref()
                    .and_then(DaemonUpdateDraftOperation::content)
            });
        if let Some(content) = content {
            content.validate()?;
        }
        Ok(())
    }

    pub(crate) fn target_id(&self) -> Option<&str> {
        self.update
            .as_ref()
            .map(DaemonUpdateDraftOperation::id)
            .or_else(|| self.rename.as_ref().map(|operation| operation.id.as_str()))
            .or_else(|| self.delete.as_ref().map(|operation| operation.id.as_str()))
            .or_else(|| self.discard.as_ref().map(|operation| operation.id.as_str()))
    }

    pub(crate) fn has_text_update(&self) -> bool {
        matches!(
            self.update.as_ref(),
            Some(DaemonUpdateDraftOperation::Text(_))
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonCreateDraftOperation {
    pub path: String,
    pub content: DaemonDraftContent,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum DaemonUpdateDraftOperation {
    Content(DaemonContentDraftUpdate),
    Text(DaemonTextDraftUpdate),
}

impl DaemonUpdateDraftOperation {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Content(update) => &update.id,
            Self::Text(update) => &update.id,
        }
    }

    pub fn content(&self) -> Option<&DaemonDraftContent> {
        match self {
            Self::Content(update) => Some(&update.content),
            Self::Text(_) => None,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), DaemonError> {
        if self.id().trim().is_empty() {
            return Err(DaemonError::InvalidRequest(
                "draft update id must not be empty".to_owned(),
            ));
        }
        let Self::Text(update) = self else {
            return Ok(());
        };
        if update.expected_hash.trim().is_empty() {
            return Err(DaemonError::InvalidRequest(
                "text replacement expected_hash must not be empty".to_owned(),
            ));
        }
        if update.replacements.is_empty() {
            return Err(DaemonError::InvalidRequest(
                "text replacement update requires at least one replacement".to_owned(),
            ));
        }
        if update
            .replacements
            .iter()
            .any(|replacement| replacement.old_text.is_empty())
        {
            return Err(DaemonError::InvalidRequest(
                "text replacement old_text must not be empty".to_owned(),
            ));
        }
        Ok(())
    }

    pub(crate) fn into_text(self) -> Option<DaemonTextDraftUpdate> {
        match self {
            Self::Text(update) => Some(update),
            Self::Content(_) => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DaemonContentDraftUpdate {
    pub id: String,
    pub content: DaemonDraftContent,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DaemonTextDraftUpdate {
    pub id: String,
    pub expected_hash: String,
    pub replacements: Vec<DaemonTextReplacement>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DaemonTextReplacement {
    pub old_text: String,
    pub new_text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonDraftContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub content: String,
}

impl DaemonDraftContent {
    pub(crate) fn from_resource(_resource: DaemonDraftResourceKind, content: String) -> Self {
        Self {
            description: None,
            content,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), DaemonError> {
        if self.content.trim().is_empty() {
            return Err(DaemonError::InvalidRequest(
                "memory content must not be empty".to_owned(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod draft_operation_validation_tests {
    use super::*;
    use crate::util::apply_exact_text_replacements;

    fn create_operation(content: DaemonDraftContent) -> DaemonDraftOperation {
        DaemonDraftOperation {
            create: Some(DaemonCreateDraftOperation {
                path: "memory/test.md".to_owned(),
                content,
                description: None,
            }),
            update: None,
            rename: None,
            delete: None,
            discard: None,
        }
    }

    #[test]
    fn rejects_blank_memory_content_before_storage() {
        let operation = create_operation(DaemonDraftContent {
            description: None,
            content: "  ".to_owned(),
        });

        assert!(operation.validate(DaemonDraftResourceKind::Memory).is_err());
    }

    #[test]
    fn accepts_non_blank_memory_content() {
        let operation = create_operation(DaemonDraftContent {
            description: None,
            content: "# Memory".to_owned(),
        });

        assert!(operation.validate(DaemonDraftResourceKind::Memory).is_ok());
    }

    #[test]
    fn rejects_non_portable_paths_before_storage() {
        for path in [
            "../outside.md",
            "memory//test.md",
            "memory/AUX.md",
            "memory/test\\file.md",
        ] {
            let mut operation = create_operation(DaemonDraftContent {
                description: None,
                content: "# Memory".to_owned(),
            });
            operation.create.as_mut().unwrap().path = path.to_owned();

            assert!(
                operation.validate(DaemonDraftResourceKind::Memory).is_err(),
                "path should be rejected: {path}"
            );
        }
    }

    #[test]
    fn exact_text_replacements_apply_atomically_against_the_original_text() {
        let result = apply_exact_text_replacements(
            "# Guide\n\nAlpha section.\n\nBeta section.\n",
            &[
                DaemonTextReplacement {
                    old_text: "Beta section.".to_owned(),
                    new_text: "Gamma section.".to_owned(),
                },
                DaemonTextReplacement {
                    old_text: "Alpha section.".to_owned(),
                    new_text: "First section.".to_owned(),
                },
            ],
        )
        .unwrap();

        assert_eq!(result, "# Guide\n\nFirst section.\n\nGamma section.\n");
    }

    #[test]
    fn exact_text_replacements_reject_missing_ambiguous_and_overlapping_matches() {
        let cases = [
            (
                "one",
                vec![DaemonTextReplacement {
                    old_text: "missing".to_owned(),
                    new_text: "replacement".to_owned(),
                }],
                "text_replacement_not_found",
            ),
            (
                "repeat repeat",
                vec![DaemonTextReplacement {
                    old_text: "repeat".to_owned(),
                    new_text: "replacement".to_owned(),
                }],
                "text_replacement_ambiguous",
            ),
            (
                "abcdef",
                vec![
                    DaemonTextReplacement {
                        old_text: "abc".to_owned(),
                        new_text: "first".to_owned(),
                    },
                    DaemonTextReplacement {
                        old_text: "bcde".to_owned(),
                        new_text: "second".to_owned(),
                    },
                ],
                "text_replacement_overlap",
            ),
        ];

        for (source, replacements, expected_code) in cases {
            let error = apply_exact_text_replacements(source, &replacements).unwrap_err();
            let actual_code = match &error {
                DaemonError::State { code, .. } => *code,
                _ => panic!("unexpected error: {error}"),
            };
            assert_eq!(actual_code, expected_code);
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonRenameDraftOperation {
    pub id: String,
    pub new_path: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonDeleteDraftOperation {
    pub id: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonDiscardDraftOperation {
    pub id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonDraftOperationResponse {
    pub local_operation_id: String,
    pub draft_id: String,
    pub queued: bool,
    pub sync_status: DraftOperationSyncStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DraftOperationSyncStatus {
    Queued,
    Syncing,
    Retrying,
    Synced,
    Failed,
}

#[derive(Clone, Debug)]
pub(crate) struct QueuedDraftOperation {
    pub(crate) local_operation_id: String,
    pub(crate) draft_id: String,
    pub(crate) project_id: String,
    pub(crate) scope: DaemonDraftScope,
    pub(crate) resource_kind: DaemonDraftResourceKind,
    pub(crate) operation_json: String,
    pub(crate) server_draft_id: Option<String>,
    pub(crate) server_version: i64,
    pub(crate) base_commit_id: Option<String>,
    pub(crate) target_id: Option<String>,
    pub(crate) path: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ServerCreateDraftRequest {
    pub(crate) daemon_installation_id: String,
    pub(crate) project_id: String,
    pub(crate) base_commit_id: Option<String>,
    pub(crate) title: String,
    pub(crate) description: Option<String>,
    pub(crate) resource: ServerDraftResourceRef,
    pub(crate) operations: Vec<ServerDraftOperationInput>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ServerDraftOperationBatchRequest {
    pub(crate) daemon_installation_id: String,
    pub(crate) operations: Vec<ServerDraftOperationBatchItem>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ServerDraftOperationBatchItem {
    pub(crate) local_operation_id: String,
    pub(crate) draft_id: String,
    pub(crate) expected_draft_version: i64,
    pub(crate) operation: ServerDraftOperationInput,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ServerDraftOperationBatchResponse {
    pub(crate) accepted_operations: Vec<String>,
    #[serde(rename = "cursor")]
    _cursor: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ServerDraftEventListResponse {
    pub(crate) events: Vec<ServerDraftEvent>,
    pub(crate) next_cursor: Option<String>,
    pub(crate) has_more: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ServerDraftEvent {
    pub(crate) event_id: String,
    pub(crate) draft_id: String,
    pub(crate) project_id: String,
    pub(crate) event_type: String,
    pub(crate) version: i64,
    pub(crate) daemon_installation_id: Option<String>,
    pub(crate) created_at: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ServerTokenRefreshResponse {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ServerDraftMutationResponse {
    pub(crate) draft: ServerDraftVersion,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ServerDraftVersion {
    pub(crate) draft_id: String,
    pub(crate) version: i64,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ServerDraftProjectionDetail {
    pub(crate) draft: ServerDraftProjection,
    pub(crate) operations: Vec<ServerDraftProjectionOperation>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ServerDraftCoordination {
    pub(crate) current_commit_id: Option<String>,
    pub(crate) freshness: DaemonDraftFreshness,
    pub(crate) has_upstream_resource_changes: bool,
    pub(crate) reconciliation: DaemonDraftReconciliationStatus,
    pub(crate) candidate_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ServerDraftProjection {
    pub(crate) draft_id: String,
    pub(crate) project_id: String,
    pub(crate) base_commit_id: Option<String>,
    pub(crate) coordination: ServerDraftCoordination,
    pub(crate) resource: ServerDraftResourceRef,
    pub(crate) status: DaemonLocalDraftStatus,
    pub(crate) version: i64,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ServerDraftProjectionOperation {
    pub(crate) operation_id: String,
    pub(crate) action: ServerDraftOperationAction,
    pub(crate) resource: ServerDraftResourceRef,
    pub(crate) content: Option<DaemonDraftContent>,
    pub(crate) new_path: Option<String>,
    pub(crate) created_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ServerDraftOperationInput {
    pub(crate) action: ServerDraftOperationAction,
    pub(crate) resource: ServerDraftResourceRef,
    pub(crate) content: Option<DaemonDraftContent>,
    pub(crate) new_path: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServerDraftOperationAction {
    Create,
    Update,
    Rename,
    Delete,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ServerDraftResourceRef {
    pub(crate) scope: DaemonDraftScope,
    pub(crate) id: Option<String>,
    pub(crate) path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ErrorEnvelope {
    pub error: ApiError,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub request_id: String,
    pub details: serde_json::Value,
}

pub(crate) fn api_error_from_daemon_error(error: DaemonError) -> ApiError {
    let details = crate::diagnostics::error_details(&error);
    let error = match error {
        DaemonError::Remote(error) => return error,
        DaemonError::Search { code, message } => {
            return ApiError {
                code,
                message,
                request_id: crate::diagnostics::request_id(),
                details: json!({}),
            };
        }
        DaemonError::State { code, message } => {
            return ApiError {
                code: code.to_owned(),
                message,
                request_id: crate::diagnostics::request_id(),
                details: json!({}),
            };
        }
        error => error,
    };
    let (code, message) = match error {
        DaemonError::InvalidConfig(message) => ("invalid_config", message),
        DaemonError::InvalidRequest(message) => ("invalid_request", message),
        DaemonError::NotFound(message) => ("not_found", message),
        DaemonError::Io(error) => ("io_error", error.to_string()),
        DaemonError::Sqlx(error) => ("local_db_error", error.to_string()),
        DaemonError::SerdeJson(error) => ("invalid_json", error.to_string()),
        DaemonError::Reqwest(error) => (
            "server_request_failed",
            if error.is_timeout() {
                "HTTP request timed out".to_owned()
            } else if error.is_connect() {
                "HTTP connection failed".to_owned()
            } else {
                error.without_url().to_string()
            },
        ),
        DaemonError::CredentialStore(error) => ("credential_store_failed", error.to_string()),
        DaemonError::Server(message) => ("server_sync_failed", message),
        DaemonError::ServerResponse { status, body } => (
            "server_request_failed",
            format!("Server request failed with status {status}: {body}"),
        ),
        DaemonError::Launchctl(message) => ("launchctl_failed", message),
        DaemonError::Ipc(message) => ("daemon_ipc_failed", message),
        DaemonError::Remote(_) => unreachable!("remote errors return above"),
        DaemonError::Search { .. } => unreachable!("search errors return above"),
        DaemonError::State { .. } => unreachable!("state errors return above"),
    };
    ApiError {
        code: code.to_owned(),
        message,
        request_id: crate::diagnostics::request_id(),
        details,
    }
}

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    SerdeJson(#[from] serde_json::Error),
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
    #[error(transparent)]
    CredentialStore(#[from] CredentialStoreError),
    #[error("server sync error: {0}")]
    Server(String),
    #[error("Server request failed with status {status}: {body}")]
    ServerResponse { status: u16, body: String },
    #[error("launchctl error: {0}")]
    Launchctl(String),
    #[error("daemon IPC error: {0}")]
    Ipc(String),
    #[error("{}: {} (request {})", .0.code, .0.message, .0.request_id)]
    Remote(ApiError),
    #[error("search error ({code}): {message}")]
    Search { code: String, message: String },
    #[error("state error ({code}): {message}")]
    State { code: &'static str, message: String },
}

impl DaemonError {
    pub(crate) fn is_retryable(&self) -> bool {
        match self {
            Self::Reqwest(_) => true,
            Self::ServerResponse { status, .. } => is_retryable_http_status(*status),
            _ => false,
        }
    }
}

#[derive(Debug)]
pub(crate) struct DraftSyncError {
    local_operation_id: String,
    message: String,
    retryable: bool,
}

impl DraftSyncError {
    pub(crate) fn new(local_operation_id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            local_operation_id: local_operation_id.into(),
            message: message.into(),
            retryable: false,
        }
    }

    pub(crate) fn from_daemon_error(
        local_operation_id: impl Into<String>,
        error: DaemonError,
    ) -> Self {
        Self {
            local_operation_id: local_operation_id.into(),
            retryable: error.is_retryable(),
            message: error.to_string(),
        }
    }

    pub(crate) fn local_operation_id(&self) -> &str {
        &self.local_operation_id
    }

    pub(crate) fn is_retryable(&self) -> bool {
        self.retryable
    }
}

impl std::fmt::Display for DraftSyncError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for DraftSyncError {}
