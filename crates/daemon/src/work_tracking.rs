use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use uuid::Uuid;

use crate::DaemonError;
use crate::search::{MemoryKind, SourceResource, SourceScope};

const LEASE_EXPIRED_REASON: &str = "lease_expired";
const AGENT_REPORT_REASON: &str = "agent_report";
const SESSION_ENDED_REASON: &str = "session_ended";
const RECOVERED_END_REASON: &str = "recovered_end";
const HOOK_END_REASON: &str = "hook";
const MAX_IDENTIFIER_BYTES: usize = 256;
const MAX_RUN_KEY_BYTES: usize = 256;
const MAX_DISPLAY_LABEL_BYTES: usize = 160;
const MAX_SUMMARY_BYTES: usize = 1_000;
const MAX_ISSUE_TITLE_BYTES: usize = 240;
const MAX_ISSUE_DESCRIPTION_BYTES: usize = 64 * 1024;
const MAX_ISSUE_CRITERION_BYTES: usize = 2_000;
const MAX_ISSUE_CRITERIA: usize = 64;
const MAX_ISSUE_EXTERNAL_REFERENCES: usize = 16;
const MAX_ISSUE_EXTERNAL_REFERENCE_URL_BYTES: usize = 2_048;
const MAX_ISSUE_DEPENDENCIES: usize = 16;
const MAX_ISSUE_BLOCKING_FACTS: usize = 16;
const MAX_ISSUE_FACT_ID_BYTES: usize = 128;
const MAX_ISSUE_FACT_DESCRIPTION_BYTES: usize = 1_000;
const MAX_ISSUE_FACT_VALUE_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IssueLifecycle {
    Open,
    Closed,
}

impl IssueLifecycle {
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }

    fn from_db(value: &str) -> Result<Self, DaemonError> {
        match value {
            "open" => Ok(Self::Open),
            "closed" => Ok(Self::Closed),
            value => Err(corrupt_run(format!("unknown Issue lifecycle {value}"))),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IssueBoardState {
    Todo,
    InProgress,
    Paused,
    InReview,
    Done,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum IssueExternalReferenceKind {
    Issue,
    PullRequest,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IssueExternalReference {
    pub kind: IssueExternalReferenceKind,
    pub url: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IssueBlockingFactKind {
    HostCapability,
    External,
}

impl IssueBlockingFactKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::HostCapability => "host_capability",
            Self::External => "external",
        }
    }

    fn from_db(value: &str) -> Result<Self, DaemonError> {
        match value {
            "host_capability" => Ok(Self::HostCapability),
            "external" => Ok(Self::External),
            value => Err(corrupt_run(format!(
                "unknown Issue blocking fact kind {value}"
            ))),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IssueBlockingFact {
    pub fact_id: String,
    pub kind: IssueBlockingFactKind,
    /// Optional condition value, e.g. the capability name for `host_capability`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub description: String,
    /// Whether the condition is currently satisfied; unsatisfied facts block.
    #[serde(default = "default_false")]
    pub satisfied: bool,
}

fn default_false() -> bool {
    false
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IssueBlockingReasonKind {
    Dependency,
    Fact,
}

/// One concrete reason why an Issue is currently blocked. `Dependency` carries
/// the unresolved prerequisite Issue; `Fact` carries the unsatisfied predicate.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IssueBlockingReason {
    pub kind: IssueBlockingReasonKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_state: Option<IssueBoardState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// The resolved state of one declared dependency, so an Agent can see whether
/// each prerequisite is already Done without another lookup.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct IssueDependencyState {
    pub issue_key: String,
    pub title: String,
    pub board_state: IssueBoardState,
}

impl IssueBoardState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::InProgress => "in_progress",
            Self::Paused => "paused",
            Self::InReview => "in_review",
            Self::Done => "done",
        }
    }

    fn from_db(value: &str) -> Result<Self, DaemonError> {
        match value {
            "todo" => Ok(Self::Todo),
            "in_progress" => Ok(Self::InProgress),
            "paused" => Ok(Self::Paused),
            "in_review" => Ok(Self::InReview),
            "done" => Ok(Self::Done),
            value => Err(corrupt_run(format!("unknown native Issue status {value}"))),
        }
    }

    fn as_open_str(self) -> Result<&'static str, DaemonError> {
        match self {
            Self::Todo => Ok("todo"),
            Self::InProgress => Ok("in_progress"),
            Self::Paused => Ok("paused"),
            Self::InReview => Ok("in_review"),
            Self::Done => Err(DaemonError::InvalidRequest(
                "Done is derived from the closed Issue path".to_owned(),
            )),
        }
    }

    fn from_open_db(value: &str) -> Result<Self, DaemonError> {
        match value {
            "todo" => Ok(Self::Todo),
            "in_progress" => Ok(Self::InProgress),
            "in_review" => Ok(Self::InReview),
            value => Err(corrupt_run(format!(
                "unknown open Issue board state {value}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum AgentRunHost {
    #[serde(rename = "codex")]
    Codex,
    #[serde(rename = "claude-code")]
    ClaudeCode,
    /// Run issued by a non-hook caller (tool call or manual operation) when
    /// no host lifecycle hook is available to create the run.
    #[serde(rename = "manual")]
    Manual,
    /// Reserved for a future Zed MCP-session run (no lifecycle hook surface).
    #[serde(rename = "zed")]
    Zed,
    /// opencode plugin-hook integration (event plugin + daemon bridge).
    #[serde(rename = "opencode")]
    Opencode,
    /// DeepSeek Harness (dsh) web sessions; runs are issued by a dsh-side
    /// client plugin forwarding lifecycle events to the daemon hook proxy.
    #[serde(rename = "dsh")]
    Dsh,
    /// Google Antigravity lifecycle hook integration.
    #[serde(rename = "antigravity")]
    Antigravity,
}

impl AgentRunHost {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
            Self::Manual => "manual",
            Self::Zed => "zed",
            Self::Opencode => "opencode",
            Self::Dsh => "dsh",
            Self::Antigravity => "antigravity",
        }
    }

    fn from_db(value: &str) -> Result<Self, DaemonError> {
        match value {
            "codex" => Ok(Self::Codex),
            "claude-code" => Ok(Self::ClaudeCode),
            "manual" => Ok(Self::Manual),
            "zed" => Ok(Self::Zed),
            "opencode" => Ok(Self::Opencode),
            "dsh" => Ok(Self::Dsh),
            "antigravity" => Ok(Self::Antigravity),
            value => Err(corrupt_run(format!("unknown AgentRun host {value}"))),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunKind {
    Root,
    Subagent,
}

impl AgentRunKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Subagent => "subagent",
        }
    }

    fn from_db(value: &str) -> Result<Self, DaemonError> {
        match value {
            "root" => Ok(Self::Root),
            "subagent" => Ok(Self::Subagent),
            value => Err(corrupt_run(format!("unknown AgentRun kind {value}"))),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunPhase {
    Running,
    Ended,
}

impl AgentRunPhase {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Ended => "ended",
        }
    }

    fn from_db(value: &str) -> Result<Self, DaemonError> {
        match value {
            "running" => Ok(Self::Running),
            "ended" => Ok(Self::Ended),
            value => Err(corrupt_run(format!("unknown AgentRun phase {value}"))),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunOutcome {
    Completed,
    Blocked,
    Failed,
    Cancelled,
    Unknown,
}

impl AgentRunOutcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }

    fn from_db(value: &str) -> Result<Self, DaemonError> {
        match value {
            "completed" => Ok(Self::Completed),
            "blocked" => Ok(Self::Blocked),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "unknown" => Ok(Self::Unknown),
            value => Err(corrupt_run(format!("unknown AgentRun outcome {value}"))),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunEventType {
    Started,
    Heartbeat,
    Ended,
    SessionEnded,
    IssueBound,
    OutcomeReported,
}

impl AgentRunEventType {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Heartbeat => "heartbeat",
            Self::Ended => "ended",
            Self::SessionEnded => "session_ended",
            Self::IssueBound => "issue_bound",
            Self::OutcomeReported => "outcome_reported",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunEventSource {
    Hook,
    Mcp,
    Desktop,
    Recovery,
}

impl AgentRunEventSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Hook => "hook",
            Self::Mcp => "mcp",
            Self::Desktop => "desktop",
            Self::Recovery => "recovery",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IssueBoardDiagnosticCode {
    MalformedPath,
    MalformedTitle,
    TitleNumberMismatch,
    DuplicateIssueNumber,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RecordAgentRunEventRequest {
    pub event_id: String,
    pub project_id: String,
    pub host: AgentRunHost,
    pub host_run_key: Option<String>,
    pub event_type: AgentRunEventType,
    pub source: AgentRunEventSource,
    pub host_session_id: Option<String>,
    pub parent_run_id: Option<String>,
    pub parent_host_run_key: Option<String>,
    pub kind: Option<AgentRunKind>,
    pub issue_key: Option<String>,
    pub outcome: Option<AgentRunOutcome>,
    pub display_label: Option<String>,
    pub summary: Option<String>,
    pub occurred_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RecordAgentRunEventResponse {
    pub run: Option<AgentRun>,
    pub affected_runs: Vec<AgentRun>,
    pub duplicate: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct StartIssueWorkRequest {
    pub project_id: String,
    pub run_id: Option<String>,
    pub issue_key: String,
    pub expected_revision: Option<i64>,
    /// Session identity for manual runs. Hook-issued runs carry
    /// host_session_id; manual runs declare a session so the daemon can
    /// enforce that one session holds at most one In Progress Issue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RequestIssueClosureRequest {
    pub project_id: String,
    pub run_id: Option<String>,
    pub issue_key: Option<String>,
    pub summary: Option<String>,
    pub expected_revision: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PauseIssueRequest {
    pub project_id: String,
    pub run_id: String,
    pub issue_key: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ResumeIssueRequest {
    pub project_id: String,
    /// Agent path: the AgentRun resuming the Issue. Human (desktop) path:
    /// omit to resume a Paused Issue without an AgentRun binding.
    #[serde(default)]
    pub run_id: Option<String>,
    pub issue_key: String,
    #[serde(default)]
    pub takeover: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationLevel {
    #[default]
    AgentSelf,
    HumanRequired,
    Mixed,
}

impl VerificationLevel {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::AgentSelf => "agent_self",
            Self::HumanRequired => "human_required",
            Self::Mixed => "mixed",
        }
    }

    pub(crate) fn from_db(value: &str) -> Result<Self, DaemonError> {
        match value {
            "agent_self" => Ok(Self::AgentSelf),
            "human_required" => Ok(Self::HumanRequired),
            "mixed" => Ok(Self::Mixed),
            _ => Err(DaemonError::InvalidConfig(format!(
                "unknown Issue verification level {value}"
            ))),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct VerificationStep {
    pub text: String,
    #[serde(default)]
    pub completed: bool,
}

/// One recorded board-state transition, e.g. `in_progress` -> `in_review`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IssueStateEvent {
    pub from_state: IssueBoardState,
    pub to_state: IssueBoardState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_by_run_id: Option<String>,
    pub occurred_at: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct CreateIssueRequest {
    pub project_id: String,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub external_references: Vec<IssueExternalReference>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub blocking_facts: Vec<IssueBlockingFact>,
    #[serde(default)]
    pub verification_level: VerificationLevel,
    #[serde(default)]
    pub verification_steps: Vec<VerificationStep>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct UpdateIssueRequest {
    pub project_id: String,
    pub issue_key: String,
    pub expected_revision: i64,
    pub title: Option<String>,
    pub description: Option<String>,
    pub acceptance_criteria: Option<Vec<String>>,
    pub external_references: Option<Vec<IssueExternalReference>>,
    pub dependencies: Option<Vec<String>>,
    pub blocking_facts: Option<Vec<IssueBlockingFact>>,
    pub verification_level: Option<VerificationLevel>,
    pub verification_steps: Option<Vec<VerificationStep>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IssueGateAction {
    ApproveClosure,
    RequestChanges,
    Reopen,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ApplyIssueGateRequest {
    pub project_id: String,
    pub issue_number: i64,
    pub expected_revision: i64,
    pub action: IssueGateAction,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SetVerificationStepCompletedRequest {
    pub project_id: String,
    pub issue_key: String,
    pub expected_revision: i64,
    pub step_index: usize,
    pub completed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct UnclaimIssueRequest {
    pub project_id: String,
    pub issue_key: String,
    pub expected_revision: i64,
    /// Agent path: the AgentRun releasing its own Issue. Human (desktop)
    /// path: omit to release a Paused or abandoned In Progress Issue back to
    /// Todo without an AgentRun binding (refused while an active run works it).
    #[serde(default)]
    pub run_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IssueRemovalAction {
    Archive,
    Delete,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RemoveIssueRequest {
    pub project_id: String,
    pub issue_number: i64,
    pub expected_revision: i64,
    pub action: IssueRemovalAction,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IssueRemovalResponse {
    pub issue_id: String,
    pub issue_key: String,
    pub action: IssueRemovalAction,
    pub removed_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IssueMutationResponse {
    pub issue_id: String,
    pub issue_key: String,
    pub board_state: IssueBoardState,
    pub revision: i64,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IssueWorkflowMutationResponse {
    pub issue_id: String,
    pub issue_key: String,
    pub board_state: IssueBoardState,
    pub state_revision: i64,
    pub state_updated_at: String,
    /// Bound AgentRun for agent-driven transitions; None for human (desktop)
    /// transitions without a run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<AgentRun>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IssueBoardListRequest {
    pub project_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IssueDetailRequest {
    pub project_id: String,
    pub issue_number: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct GetIssueRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IssueDetailResponse {
    pub issue: IssueBoardCard,
    pub body: String,
    pub acceptance_criteria: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ExportIssueRequest {
    pub project_id: String,
    pub issue_key: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ExportIssueResponse {
    pub issue_key: String,
    pub filename: String,
    pub markdown: String,
}

/// Renders a structured Issue as a stable, portable Markdown snapshot.
/// The output is deterministic for a given Issue state, so repeated exports
/// produce the same document (suitable for version control).
pub fn render_issue_markdown(
    issue: &IssueBoardCard,
    body: &str,
    acceptance_criteria: &[String],
) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", issue.title));
    out.push_str(&format!("- **Issue**: {}\n", issue.issue_key));
    out.push_str(&format!("- **Status**: {}\n", issue.board_state.as_str()));
    if let Some(closure_summary) = &issue.closure_summary {
        out.push_str(&format!("- **Closure**: {}\n", closure_summary));
    }
    if let Some(created_at) = &issue.created_at {
        out.push_str(&format!("- **Created**: {}\n", created_at));
    }
    if let Some(started_at) = &issue.started_at {
        out.push_str(&format!("- **Started**: {}\n", started_at));
    }
    if let Some(closed_at) = &issue.closed_at {
        out.push_str(&format!("- **Closed**: {}\n", closed_at));
    }
    out.push('\n');
    let body = body.trim();
    if !body.is_empty() {
        out.push_str("## 描述\n\n");
        out.push_str(body);
        out.push_str("\n\n");
    }
    if !acceptance_criteria.is_empty() {
        out.push_str("## 验收标准\n\n");
        for criteria in acceptance_criteria {
            out.push_str(&format!("- [ ] {}\n", criteria));
        }
        out.push('\n');
    }
    out.push_str("---\n");
    out
}

/// Stable filename for an exported Issue: ISSUE-<NNN>.md
pub fn issue_export_filename(issue_number: i64) -> String {
    format!("ISSUE-{issue_number:03}.md")
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IssueBoardResponse {
    pub project_id: String,
    pub effective_hash: String,
    pub issues: Vec<IssueBoardCard>,
    #[serde(default)]
    pub claims: Vec<IssueClaim>,
    pub unlinked_runs: Vec<AgentRun>,
    pub diagnostics: Vec<IssueBoardDiagnostic>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IssueMember {
    pub user_id: String,
    pub email: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub role: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IssueClaim {
    pub project_id: String,
    pub issue_id: String,
    pub issue_key: String,
    pub run_id: String,
    pub claimant: IssueMember,
    pub claimed_at: String,
    pub lease_expires_at: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ServerIssueClaimListResponse {
    pub(crate) items: Vec<IssueClaim>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct AcquireServerIssueClaimRequest {
    pub(crate) issue_key: String,
    pub(crate) run_id: String,
    pub(crate) lease_expires_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ReleaseServerIssueClaimRequest {
    pub(crate) run_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ServerKanbanIssueSnapshot {
    pub(crate) issue: IssueBoardCard,
    pub(crate) acceptance_criteria: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ServerKanbanIssue {
    pub(crate) project_id: String,
    pub(crate) issue_id: String,
    pub(crate) issue_number: i64,
    pub(crate) assignee: IssueMember,
    pub(crate) content_revision: i64,
    pub(crate) payload: ServerKanbanIssueSnapshot,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ServerKanbanIssueListResponse {
    pub(crate) items: Vec<ServerKanbanIssue>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ImportServerKanbanIssue {
    pub(crate) issue_id: String,
    pub(crate) issue_number: i64,
    pub(crate) content_revision: i64,
    pub(crate) payload: ServerKanbanIssueSnapshot,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ImportServerKanbanIssuesRequest {
    pub(crate) items: Vec<ImportServerKanbanIssue>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct UpdateServerKanbanIssueRequest {
    pub(crate) expected_content_revision: i64,
    pub(crate) content_revision: i64,
    pub(crate) payload: ServerKanbanIssueSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NativeIssue {
    pub(crate) issue_id: String,
    pub(crate) project_id: String,
    pub(crate) issue_number: i64,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) acceptance_criteria: Vec<String>,
    pub(crate) external_references: Vec<IssueExternalReference>,
    pub(crate) dependencies: Vec<i64>,
    pub(crate) blocking_facts: Vec<IssueBlockingFact>,
    pub(crate) board_state: IssueBoardState,
    pub(crate) revision: i64,
    pub(crate) changed_by_run_id: Option<String>,
    pub(crate) closure_summary: Option<String>,
    pub(crate) created_at: String,
    pub(crate) started_at: Option<String>,
    pub(crate) updated_at: String,
    pub(crate) closed_at: Option<String>,
    pub(crate) archived_at: Option<String>,
    pub(crate) verification_level: VerificationLevel,
    pub(crate) verification_steps: Vec<VerificationStep>,
    pub(crate) state_events: Vec<IssueStateEvent>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IssueBoardCard {
    pub issue_id: String,
    pub project_id: String,
    pub issue_number: i64,
    pub issue_key: String,
    pub resource_id: String,
    pub path: String,
    pub lifecycle: IssueLifecycle,
    pub title: String,
    pub description: String,
    pub description_excerpt: String,
    pub external_references: Vec<IssueExternalReference>,
    pub found_at: Option<String>,
    pub created_at: Option<String>,
    pub started_at: Option<String>,
    pub closed_at: Option<String>,
    pub archived_at: Option<String>,
    pub content_hash: String,
    pub source_commit_id: Option<String>,
    pub draft_id: Option<String>,
    pub draft_revision: Option<String>,
    pub board_state: IssueBoardState,
    pub state_revision: i64,
    pub state_updated_at: Option<String>,
    pub closure_summary: Option<String>,
    #[serde(default)]
    pub assignee: Option<IssueMember>,
    pub is_stale: bool,
    pub blocked: bool,
    pub blocking_reasons: Vec<IssueBlockingReason>,
    pub dependencies: Vec<IssueDependencyState>,
    pub blocking_facts: Vec<IssueBlockingFact>,
    pub active_runs: Vec<AgentRun>,
    pub latest_run: Option<AgentRun>,
    pub changed_by_run_id: Option<String>,
    pub verification_level: VerificationLevel,
    pub verification_steps: Vec<VerificationStep>,
    pub state_events: Vec<IssueStateEvent>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentRun {
    pub run_id: String,
    pub project_id: String,
    pub issue_number: Option<i64>,
    pub host: AgentRunHost,
    pub host_run_key: String,
    pub host_session_id: Option<String>,
    pub parent_run_id: Option<String>,
    pub kind: AgentRunKind,
    pub phase: AgentRunPhase,
    pub outcome: Option<AgentRunOutcome>,
    pub end_reason: Option<String>,
    pub display_label: Option<String>,
    pub summary: Option<String>,
    pub revision: i64,
    pub started_at: String,
    pub last_seen_at: String,
    pub lease_expires_at: String,
    pub ended_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IssueBoardDiagnostic {
    pub resource_id: String,
    pub path: String,
    pub code: IssueBoardDiagnosticCode,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IssueRunProjection {
    pub(crate) active_runs: Vec<AgentRun>,
    pub(crate) latest_run: Option<AgentRun>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IssueWorkflowState {
    pub(crate) issue_number: i64,
    pub(crate) board_state: IssueBoardState,
    pub(crate) observed_lifecycle: IssueLifecycle,
    pub(crate) revision: i64,
    pub(crate) updated_at: String,
    pub(crate) changed_by_run_id: Option<String>,
    pub(crate) summary: Option<String>,
}

pub(crate) async fn migrate(pool: &SqlitePool) -> Result<(), DaemonError> {
    for statement in [
        "CREATE TABLE IF NOT EXISTS agent_runs (
            run_id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            issue_number BIGINT CHECK (issue_number IS NULL OR issue_number > 0),
            host TEXT NOT NULL CHECK (host IN ('codex', 'claude-code', 'zed', 'manual', 'opencode', 'dsh')),
            host_run_key TEXT NOT NULL,
            host_session_id TEXT,
            parent_run_id TEXT REFERENCES agent_runs(run_id),
            kind TEXT NOT NULL CHECK (kind IN ('root', 'subagent')),
            phase TEXT NOT NULL CHECK (phase IN ('running', 'ended')),
            outcome TEXT CHECK (outcome IN ('completed', 'blocked', 'failed', 'cancelled', 'unknown')),
            end_reason TEXT,
            display_label TEXT,
            summary TEXT,
            revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
            start_observed INTEGER NOT NULL DEFAULT 1 CHECK (start_observed IN (0, 1)),
            started_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL,
            lease_expires_at TEXT NOT NULL,
            ended_at TEXT,
            UNIQUE (project_id, host, host_run_key)
        )",
        "CREATE INDEX IF NOT EXISTS idx_agent_runs_project_issue_latest
         ON agent_runs (project_id, issue_number, last_seen_at DESC, run_id DESC)",
        "CREATE INDEX IF NOT EXISTS idx_agent_runs_running_lease
         ON agent_runs (phase, lease_expires_at)",
        "CREATE INDEX IF NOT EXISTS idx_agent_runs_project_session
         ON agent_runs (project_id, host, host_session_id, phase)",
        "CREATE TABLE IF NOT EXISTS agent_run_events (
            event_id TEXT PRIMARY KEY,
            event_fingerprint TEXT NOT NULL,
            run_id TEXT REFERENCES agent_runs(run_id) ON DELETE CASCADE,
            host_session_id TEXT,
            event_type TEXT NOT NULL CHECK (event_type IN (
                'started', 'heartbeat', 'ended', 'session_ended',
                'issue_bound', 'outcome_reported'
            )),
            source TEXT NOT NULL CHECK (source IN ('hook', 'mcp', 'desktop', 'recovery')),
            issue_number BIGINT CHECK (issue_number IS NULL OR issue_number > 0),
            outcome TEXT CHECK (outcome IN ('completed', 'blocked', 'failed', 'cancelled', 'unknown')),
            summary TEXT,
            occurred_at TEXT NOT NULL,
            received_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            CHECK (run_id IS NOT NULL OR (event_type = 'session_ended' AND host_session_id IS NOT NULL))
        )",
        "CREATE INDEX IF NOT EXISTS idx_agent_run_events_run_occurred
         ON agent_run_events (run_id, occurred_at DESC, event_id DESC)",
        "CREATE TABLE IF NOT EXISTS issue_workflow_states (
            project_id TEXT NOT NULL,
            issue_number BIGINT NOT NULL CHECK (issue_number > 0),
            open_state TEXT NOT NULL CHECK (open_state IN (
                'todo', 'in_progress', 'paused', 'in_review'
            )),
            observed_lifecycle TEXT NOT NULL CHECK (observed_lifecycle IN ('open', 'closed')),
            revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
            changed_by_run_id TEXT REFERENCES agent_runs(run_id),
            summary TEXT,
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            PRIMARY KEY (project_id, issue_number)
        )",
        "CREATE INDEX IF NOT EXISTS idx_issue_workflow_states_project_state
         ON issue_workflow_states (project_id, open_state, updated_at DESC)",
        "CREATE TABLE IF NOT EXISTS native_issues (
            issue_id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            issue_number BIGINT NOT NULL CHECK (issue_number BETWEEN 1 AND 999),
            title TEXT NOT NULL,
            description TEXT NOT NULL,
            acceptance_criteria_json TEXT NOT NULL DEFAULT '[]',
            external_references_json TEXT NOT NULL DEFAULT '[]',
            status TEXT NOT NULL CHECK (status IN (
                'todo', 'in_progress', 'paused', 'in_review', 'done'
            )),
            revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
            changed_by_run_id TEXT REFERENCES agent_runs(run_id),
            closure_summary TEXT,
            verification_level TEXT NOT NULL DEFAULT 'agent_self' CHECK (verification_level IN ('agent_self', 'human_required', 'mixed')),
            verification_steps_json TEXT NOT NULL DEFAULT '[]',
            created_at TEXT NOT NULL,
            started_at TEXT,
            updated_at TEXT NOT NULL,
            closed_at TEXT,
            archived_at TEXT,
            UNIQUE (project_id, issue_number)
        )",
        "CREATE INDEX IF NOT EXISTS idx_native_issues_project_status
         ON native_issues (project_id, status, issue_number)",
        "CREATE TABLE IF NOT EXISTS native_issue_imports (
            project_id TEXT PRIMARY KEY,
            imported_at TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS issue_dependencies (
            project_id TEXT NOT NULL,
            issue_number BIGINT NOT NULL CHECK (issue_number BETWEEN 1 AND 999),
            depends_on_number BIGINT NOT NULL CHECK (depends_on_number BETWEEN 1 AND 999),
            created_at TEXT NOT NULL,
            PRIMARY KEY (project_id, issue_number, depends_on_number),
            CHECK (issue_number <> depends_on_number),
            FOREIGN KEY (project_id, issue_number) REFERENCES native_issues(project_id, issue_number) ON DELETE CASCADE,
            FOREIGN KEY (project_id, depends_on_number) REFERENCES native_issues(project_id, issue_number) ON DELETE CASCADE
        )",
        "CREATE INDEX IF NOT EXISTS idx_issue_dependencies_depends_on
         ON issue_dependencies (project_id, depends_on_number, issue_number)",
        "CREATE TABLE IF NOT EXISTS issue_blocking_facts (
            project_id TEXT NOT NULL,
            issue_number BIGINT NOT NULL CHECK (issue_number BETWEEN 1 AND 999),
            fact_id TEXT NOT NULL,
            kind TEXT NOT NULL CHECK (kind IN ('host_capability', 'external')),
            value TEXT,
            description TEXT NOT NULL,
            satisfied INTEGER NOT NULL CHECK (satisfied IN (0, 1)),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (project_id, issue_number, fact_id),
            FOREIGN KEY (project_id, issue_number) REFERENCES native_issues(project_id, issue_number) ON DELETE CASCADE
        )",
        "CREATE INDEX IF NOT EXISTS idx_issue_blocking_facts_issue
         ON issue_blocking_facts (project_id, issue_number)",
        "CREATE TABLE IF NOT EXISTS issue_state_events (
            event_id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            issue_number BIGINT NOT NULL CHECK (issue_number BETWEEN 1 AND 999),
            from_state TEXT NOT NULL,
            to_state TEXT NOT NULL,
            changed_by_run_id TEXT REFERENCES agent_runs(run_id),
            occurred_at TEXT NOT NULL,
            FOREIGN KEY (project_id, issue_number)
                REFERENCES native_issues(project_id, issue_number) ON DELETE CASCADE
        )",
        "CREATE INDEX IF NOT EXISTS idx_issue_state_events_issue_occurred
         ON issue_state_events (project_id, issue_number, occurred_at ASC, event_id ASC)",
    ] {
        sqlx::query(statement).execute(pool).await?;
    }
    let event_columns = sqlx::query("PRAGMA table_info(agent_run_events)")
        .fetch_all(pool)
        .await?;
    if !event_columns
        .iter()
        .any(|row| row.get::<String, _>("name") == "event_fingerprint")
    {
        sqlx::query(
            "ALTER TABLE agent_run_events
             ADD COLUMN event_fingerprint TEXT NOT NULL DEFAULT 'legacy'",
        )
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub(crate) async fn record_agent_run_event(
    pool: &SqlitePool,
    request: RecordAgentRunEventRequest,
) -> Result<RecordAgentRunEventResponse, DaemonError> {
    validate_record_request(&request)?;
    let event_fingerprint = canonical_json_fingerprint(&request)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let existing_event = load_existing_event(&mut tx, &request.event_id).await?;
    if let Some(existing_event) = &existing_event
        && existing_event.event_fingerprint != event_fingerprint
    {
        return Err(run_conflict(format!(
            "event_id {} was already used for a different request",
            request.event_id
        )));
    }
    if request.event_type != AgentRunEventType::SessionEnded
        && let Some(existing_event) = &existing_event
    {
        let response =
            duplicate_run_event_response(&mut tx, &request, existing_event.run_id.as_deref())
                .await?;
        tx.commit().await?;
        return Ok(response);
    }
    let duplicate = existing_event.is_some();
    let (received_at, lease_expires_at) = db_clock(&mut tx).await?;
    let occurred_at =
        normalize_occurred_at(&mut tx, request.occurred_at.as_deref(), &received_at).await?;

    if request.event_type == AgentRunEventType::SessionEnded {
        let host_session_id = request
            .host_session_id
            .as_deref()
            .expect("validated SessionEnd host_session_id");
        let rows = sqlx::query(
            "SELECT run_id, project_id, issue_number, host, host_run_key, host_session_id,
                    parent_run_id, kind, phase, outcome, end_reason, display_label, summary,
                    revision, started_at, last_seen_at, lease_expires_at, ended_at
             FROM agent_runs
             WHERE project_id = $1 AND host = $2 AND host_session_id = $3 AND phase = 'running'
             ORDER BY last_seen_at DESC, run_id DESC",
        )
        .bind(&request.project_id)
        .bind(request.host.as_str())
        .bind(host_session_id)
        .fetch_all(&mut *tx)
        .await?;
        let mut affected_runs = Vec::with_capacity(rows.len());
        for row in rows {
            let mut run = agent_run_from_row(&row)?;
            run.phase = AgentRunPhase::Ended;
            run.outcome = Some(request.outcome.unwrap_or(AgentRunOutcome::Unknown));
            run.end_reason = Some(SESSION_ENDED_REASON.to_owned());
            run.revision += 1;
            run.last_seen_at = received_at.clone();
            run.ended_at = Some(occurred_at.clone());
            update_run(&mut tx, &run).await?;
            affected_runs.push(run);
        }
        if !duplicate {
            insert_event(
                &mut tx,
                &request.event_id,
                Some(&event_fingerprint),
                None,
                request.host_session_id.as_deref(),
                request.event_type,
                request.source,
                None,
                request.outcome.or(Some(AgentRunOutcome::Unknown)),
                request.summary.as_deref(),
                &occurred_at,
            )
            .await?;
        }
        tx.commit().await?;
        return Ok(RecordAgentRunEventResponse {
            run: None,
            affected_runs,
            duplicate,
        });
    }

    let host_run_key = request
        .host_run_key
        .as_deref()
        .expect("validated host_run_key");
    let existing =
        load_run_by_host_key(&mut tx, &request.project_id, request.host, host_run_key).await?;
    let kind = request
        .kind
        .or(existing.as_ref().map(|run| run.kind))
        .unwrap_or(AgentRunKind::Root);
    if let Some(existing) = &existing
        && existing.kind != kind
    {
        return Err(run_conflict(format!(
            "AgentRun {} is already recorded as {:?}",
            existing.run_id, existing.kind
        )));
    }
    let mut affected_runs = if request.source == AgentRunEventSource::Hook
        && request.event_type == AgentRunEventType::Started
        && kind == AgentRunKind::Root
        && existing.is_none()
        && let Some(host_session_id) = request.host_session_id.as_deref()
    {
        recover_prior_root_runs_for_new_hook_turn(
            &mut tx,
            &request.project_id,
            request.host,
            host_session_id,
            host_run_key,
            &received_at,
            &occurred_at,
        )
        .await?
    } else {
        Vec::new()
    };
    let parent = resolve_parent_run(&mut tx, &request, kind).await?;
    let explicit_issue_number = request
        .issue_key
        .as_deref()
        .map(parse_issue_reference)
        .transpose()?;
    let inherited_issue_number = parent.as_ref().and_then(|run| run.issue_number);
    let requested_issue_number = explicit_issue_number.or(inherited_issue_number);

    let run = if let Some(mut run) = existing {
        if let (Some(current), Some(requested)) = (run.issue_number, requested_issue_number)
            && current != requested
        {
            return Err(run_conflict(format!(
                "AgentRun {} is already bound to ISSUE-{current:03}",
                run.run_id
            )));
        }
        if run.host_session_id.is_none() {
            run.host_session_id = request.host_session_id.clone();
        }
        if run.parent_run_id.is_none() {
            run.parent_run_id = parent.as_ref().map(|parent| parent.run_id.clone());
        }
        if run.issue_number.is_none() {
            run.issue_number = requested_issue_number;
        }
        if let Some(label) = &request.display_label {
            run.display_label = Some(label.clone());
        }
        if let Some(summary) = &request.summary {
            run.summary = Some(summary.clone());
        }
        match request.event_type {
            AgentRunEventType::Started | AgentRunEventType::Heartbeat
                if run.phase == AgentRunPhase::Running =>
            {
                run.last_seen_at = received_at.clone();
                run.lease_expires_at = lease_expires_at.clone();
                run.revision += 1;
                update_run(&mut tx, &run).await?;
            }
            AgentRunEventType::Ended => {
                run.phase = AgentRunPhase::Ended;
                if run.end_reason.as_deref() != Some(AGENT_REPORT_REASON) {
                    if let Some(outcome) = request.outcome {
                        run.outcome = Some(outcome);
                    }
                    run.end_reason = Some(HOOK_END_REASON.to_owned());
                }
                run.last_seen_at = received_at.clone();
                if run.ended_at.is_none() {
                    run.ended_at = Some(occurred_at.clone());
                }
                run.revision += 1;
                update_run(&mut tx, &run).await?;
            }
            _ => {}
        }
        run
    } else {
        let ended = request.event_type == AgentRunEventType::Ended;
        let run = AgentRun {
            run_id: format!("arun_{}", Uuid::new_v4().simple()),
            project_id: request.project_id.clone(),
            issue_number: requested_issue_number,
            host: request.host,
            host_run_key: host_run_key.to_owned(),
            host_session_id: request.host_session_id.clone(),
            parent_run_id: parent.as_ref().map(|parent| parent.run_id.clone()),
            kind,
            phase: if ended {
                AgentRunPhase::Ended
            } else {
                AgentRunPhase::Running
            },
            outcome: ended.then_some(request.outcome).flatten(),
            end_reason: ended.then(|| RECOVERED_END_REASON.to_owned()),
            display_label: request.display_label.clone(),
            summary: request.summary.clone(),
            revision: 1,
            started_at: occurred_at.clone(),
            last_seen_at: received_at.clone(),
            lease_expires_at,
            ended_at: ended.then(|| occurred_at.clone()),
        };
        insert_run(
            &mut tx,
            &run,
            request.event_type == AgentRunEventType::Started,
        )
        .await?;
        run
    };
    insert_event(
        &mut tx,
        &request.event_id,
        Some(&event_fingerprint),
        Some(&run.run_id),
        request.host_session_id.as_deref(),
        request.event_type,
        request.source,
        run.issue_number,
        request.outcome,
        request.summary.as_deref(),
        &occurred_at,
    )
    .await?;
    tx.commit().await?;
    let response_run = run.clone();
    affected_runs.push(run);
    Ok(RecordAgentRunEventResponse {
        run: Some(response_run),
        affected_runs,
        duplicate: false,
    })
}

#[allow(clippy::too_many_arguments)]
async fn recover_prior_root_runs_for_new_hook_turn(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    host: AgentRunHost,
    host_session_id: &str,
    new_host_run_key: &str,
    received_at: &str,
    occurred_at: &str,
) -> Result<Vec<AgentRun>, DaemonError> {
    let rows = sqlx::query(
        "SELECT run_id, project_id, issue_number, host, host_run_key, host_session_id,
                parent_run_id, kind, phase, outcome, end_reason, display_label, summary,
                revision, started_at, last_seen_at, lease_expires_at, ended_at
         FROM agent_runs
         WHERE project_id = $1 AND host = $2 AND host_session_id = $3
           AND host_run_key <> $4 AND kind = 'root' AND phase = 'running'
         ORDER BY last_seen_at DESC, run_id DESC",
    )
    .bind(project_id)
    .bind(host.as_str())
    .bind(host_session_id)
    .bind(new_host_run_key)
    .fetch_all(&mut **tx)
    .await?;

    let mut recovered = Vec::with_capacity(rows.len());
    for row in rows {
        let mut run = agent_run_from_row(&row)?;
        run.phase = AgentRunPhase::Ended;
        if run.end_reason.as_deref() != Some(AGENT_REPORT_REASON) {
            run.outcome = Some(AgentRunOutcome::Unknown);
            run.end_reason = Some(RECOVERED_END_REASON.to_owned());
        }
        run.revision += 1;
        run.last_seen_at = received_at.to_owned();
        if run.ended_at.is_none() {
            run.ended_at = Some(occurred_at.to_owned());
        }
        update_run(tx, &run).await?;
        insert_event(
            tx,
            &format!("arevt_{}", Uuid::new_v4().simple()),
            None,
            Some(&run.run_id),
            run.host_session_id.as_deref(),
            AgentRunEventType::Ended,
            AgentRunEventSource::Recovery,
            run.issue_number,
            run.outcome,
            None,
            occurred_at,
        )
        .await?;
        recovered.push(run);
    }
    Ok(recovered)
}

pub(crate) async fn start_issue_work(
    pool: &SqlitePool,
    request: StartIssueWorkRequest,
) -> Result<IssueWorkflowMutationResponse, DaemonError> {
    validate_required("project_id", &request.project_id, MAX_IDENTIFIER_BYTES)?;
    if let Some(run_id) = &request.run_id {
        validate_required("run_id", run_id, MAX_IDENTIFIER_BYTES)?;
    }
    let issue_number = parse_issue_reference(&request.issue_key)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let (now, _) = db_clock(&mut tx).await?;
    let mut run = match &request.run_id {
        Some(run_id) => {
            let run = load_agent_run_for_project_tx(&mut tx, &request.project_id, run_id)
                .await?
                .ok_or_else(|| DaemonError::NotFound(format!("AgentRun {run_id}")))?;
            if let Some(current) = run.issue_number
                && current != issue_number
            {
                return Err(run_conflict(format!(
                    "AgentRun {} is already bound to ISSUE-{current:03}",
                    run.run_id
                )));
            }
            run
        }
        None => {
            return Err(DaemonError::InvalidRequest(
                "run_id is required: AgentRuns must be issued by a host lifecycle hook, \
                 Agents cannot mint their own identity"
                    .to_owned(),
            ));
        }
    };
    let current_issue = load_native_issue_tx(&mut tx, &request.project_id, issue_number)
        .await?
        .ok_or_else(|| DaemonError::NotFound(format!("ISSUE-{issue_number:03}")))?;
    if current_issue.board_state == IssueBoardState::Done {
        return Err(DaemonError::InvalidRequest(format!(
            "ISSUE-{issue_number:03} is Done; the user must reopen it first"
        )));
    }
    let is_idempotent = run.issue_number == Some(issue_number)
        && current_issue.board_state == IssueBoardState::InProgress
        && current_issue.changed_by_run_id.as_deref() == Some(run.run_id.as_str());
    if !is_idempotent {
        let other_active_runs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM agent_runs
             WHERE project_id = $1 AND issue_number = $2 AND run_id <> $3
               AND phase = 'running' AND lease_expires_at > $4",
        )
        .bind(&request.project_id)
        .bind(issue_number)
        .bind(&run.run_id)
        .bind(&now)
        .fetch_one(&mut *tx)
        .await?;
        if other_active_runs > 0 {
            return Err(run_conflict(format!(
                "ISSUE-{issue_number:03} is already being worked on by {other_active_runs} other active AgentRun(s)"
            )));
        }
        // One session holds at most one In Progress Issue. The session key is
        // the run's host_session_id (hook-issued) or the declared manual
        // session_id. Reject starting another Issue while that session holds
        // an In Progress Issue elsewhere.
        let session_key = run
            .host_session_id
            .as_deref()
            .or(request.session_id.as_deref());
        if let Some(session_key) = session_key {
            let held_issue: Option<i64> = sqlx::query_scalar(
                "SELECT n.issue_number
                 FROM agent_runs r
                 JOIN native_issues n
                   ON n.project_id = r.project_id AND n.issue_number = r.issue_number
                 WHERE r.project_id = $1
                   AND r.host_session_id = $3
                   AND r.phase = 'running'
                   AND r.lease_expires_at > $4
                   AND n.status = 'in_progress'
                 ORDER BY n.issue_number LIMIT 1",
            )
            .bind(&request.project_id)
            .bind(&run.run_id)
            .bind(session_key)
            .bind(&now)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(held_issue) = held_issue {
                return Err(run_conflict(format!(
                    "this session is already working on ISSUE-{held_issue:03} (In Progress); pause or request closure for it before starting ISSUE-{issue_number:03}"
                )));
            }
        }
        match request.expected_revision {
            Some(expected_revision) => ensure_revision(&run, expected_revision)?,
            None if request.run_id.is_some() => {
                return Err(DaemonError::InvalidRequest(
                    "expected_revision is required when run_id is provided".to_owned(),
                ));
            }
            None => {}
        }
    }
    if run.issue_number.is_none() {
        run.issue_number = Some(issue_number);
        run.revision += 1;
        run.last_seen_at = now.clone();
        update_run(&mut tx, &run).await?;
        insert_event(
            &mut tx,
            &format!("arevt_{}", Uuid::new_v4().simple()),
            None,
            Some(&run.run_id),
            run.host_session_id.as_deref(),
            AgentRunEventType::IssueBound,
            AgentRunEventSource::Mcp,
            Some(issue_number),
            None,
            None,
            &now,
        )
        .await?;
        bind_unlinked_children(&mut tx, &run, issue_number, &now).await?;
    }
    let issue = set_native_issue_state_tx(
        &mut tx,
        &request.project_id,
        issue_number,
        IssueBoardState::InProgress,
        Some(&run.run_id),
        None,
        &now,
    )
    .await?;
    tx.commit().await?;
    Ok(issue_workflow_mutation_response(Some(run), &issue))
}

pub(crate) async fn request_issue_closure(
    pool: &SqlitePool,
    request: RequestIssueClosureRequest,
) -> Result<IssueWorkflowMutationResponse, DaemonError> {
    validate_required("project_id", &request.project_id, MAX_IDENTIFIER_BYTES)?;
    if let Some(run_id) = &request.run_id {
        validate_required("run_id", run_id, MAX_IDENTIFIER_BYTES)?;
    }
    validate_optional("summary", request.summary.as_deref(), MAX_SUMMARY_BYTES)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let (now, _) = db_clock(&mut tx).await?;

    let (issue_number, run) = match &request.run_id {
        Some(run_id) => {
            let run = load_agent_run_for_project_tx(&mut tx, &request.project_id, run_id)
                .await?
                .ok_or_else(|| DaemonError::NotFound(format!("AgentRun {run_id}")))?;
            if run.kind != AgentRunKind::Root {
                return Err(DaemonError::InvalidRequest(
                    "only a root AgentRun can request Issue closure".to_owned(),
                ));
            }
            let issue_number = run.issue_number.ok_or_else(|| {
                DaemonError::InvalidRequest(
                    "the current AgentRun must start work on an Issue before requesting closure"
                        .to_owned(),
                )
            })?;
            (issue_number, run)
        }
        None => {
            return Err(DaemonError::InvalidRequest(
                "run_id is required: AgentRuns must be issued by a host lifecycle hook, \
                 Agents cannot mint their own identity"
                    .to_owned(),
            ));
        }
    };

    let current_issue = load_native_issue_tx(&mut tx, &request.project_id, issue_number)
        .await?
        .ok_or_else(|| DaemonError::NotFound(format!("ISSUE-{issue_number:03}")))?;
    let is_idempotent = current_issue.board_state == IssueBoardState::InReview
        && current_issue.changed_by_run_id.as_deref() == Some(run.run_id.as_str())
        && current_issue.closure_summary == request.summary;
    if is_idempotent {
        tx.commit().await?;
        return Ok(issue_workflow_mutation_response(Some(run), &current_issue));
    }
    match request.expected_revision {
        Some(expected_revision) => ensure_revision(&run, expected_revision)?,
        None => {
            return Err(DaemonError::InvalidRequest(
                "expected_revision is required when run_id is provided".to_owned(),
            ));
        }
    }
    if current_issue.board_state != IssueBoardState::InProgress
        || current_issue.changed_by_run_id.as_deref() != Some(run.run_id.as_str())
    {
        return Err(run_conflict(format!(
            "ISSUE-{issue_number:03} is not in progress for AgentRun {}",
            run.run_id
        )));
    }
    if current_issue.verification_level != VerificationLevel::AgentSelf
        && current_issue.verification_steps.is_empty()
    {
        return Err(DaemonError::InvalidRequest(format!(
            "ISSUE-{issue_number:03} requires human verification ({}) but has no \
             verification_steps; add them with update_issue before requesting closure",
            current_issue.verification_level.as_str()
        )));
    }
    let other_active_runs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_runs
         WHERE project_id = $1 AND issue_number = $2 AND run_id <> $3
           AND phase = 'running' AND lease_expires_at > $4",
    )
    .bind(&request.project_id)
    .bind(issue_number)
    .bind(&run.run_id)
    .bind(&now)
    .fetch_one(&mut *tx)
    .await?;
    if other_active_runs > 0 {
        return Err(run_conflict(format!(
            "ISSUE-{issue_number:03} still has {other_active_runs} other active AgentRun(s)"
        )));
    }
    let issue = set_native_issue_state_tx(
        &mut tx,
        &request.project_id,
        issue_number,
        IssueBoardState::InReview,
        Some(&run.run_id),
        request.summary.as_deref(),
        &now,
    )
    .await?;
    tx.commit().await?;
    Ok(issue_workflow_mutation_response(Some(run), &issue))
}

/// Pauses an In Progress Issue held by the given run, moving it to Paused.
/// The Issue must be bound to the run and in In Progress; the run stays
/// running so it can resume later.
pub(crate) async fn pause_issue_work(
    pool: &SqlitePool,
    request: PauseIssueRequest,
) -> Result<IssueWorkflowMutationResponse, DaemonError> {
    validate_required("project_id", &request.project_id, MAX_IDENTIFIER_BYTES)?;
    validate_required("run_id", &request.run_id, MAX_IDENTIFIER_BYTES)?;
    let issue_number = parse_issue_reference(&request.issue_key)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let (now, _) = db_clock(&mut tx).await?;
    let run = load_agent_run_for_project_tx(&mut tx, &request.project_id, &request.run_id)
        .await?
        .ok_or_else(|| DaemonError::NotFound(format!("AgentRun {}", request.run_id)))?;
    if run.issue_number != Some(issue_number) {
        return Err(run_conflict(format!(
            "AgentRun {} is not bound to ISSUE-{issue_number:03}",
            run.run_id
        )));
    }
    let current_issue = load_native_issue_tx(&mut tx, &request.project_id, issue_number)
        .await?
        .ok_or_else(|| DaemonError::NotFound(format!("ISSUE-{issue_number:03}")))?;
    if current_issue.board_state != IssueBoardState::InProgress {
        return Err(run_conflict(format!(
            "ISSUE-{issue_number:03} is not In Progress"
        )));
    }
    let issue = set_native_issue_state_tx(
        &mut tx,
        &request.project_id,
        issue_number,
        IssueBoardState::Paused,
        Some(&run.run_id),
        None,
        &now,
    )
    .await?;
    tx.commit().await?;
    Ok(issue_workflow_mutation_response(Some(run), &issue))
}

/// Resumes a Paused Issue. The agent path requires the run that paused it
/// (changed_by_run_id) or an explicit takeover; the human (desktop) path
/// omits the run and resumes without an AgentRun binding.
pub(crate) async fn resume_issue_work(
    pool: &SqlitePool,
    request: ResumeIssueRequest,
) -> Result<IssueWorkflowMutationResponse, DaemonError> {
    validate_required("project_id", &request.project_id, MAX_IDENTIFIER_BYTES)?;
    if let Some(run_id) = request.run_id.as_deref() {
        validate_required("run_id", run_id, MAX_IDENTIFIER_BYTES)?;
    }
    let issue_number = parse_issue_reference(&request.issue_key)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let (now, _) = db_clock(&mut tx).await?;
    let current_issue = load_native_issue_tx(&mut tx, &request.project_id, issue_number)
        .await?
        .ok_or_else(|| DaemonError::NotFound(format!("ISSUE-{issue_number:03}")))?;
    if current_issue.board_state != IssueBoardState::Paused {
        return Err(run_conflict(format!(
            "ISSUE-{issue_number:03} is not Paused"
        )));
    }
    let run = match request.run_id.as_deref() {
        Some(run_id) => {
            let run = load_agent_run_for_project_tx(&mut tx, &request.project_id, run_id)
                .await?
                .ok_or_else(|| DaemonError::NotFound(format!("AgentRun {run_id}")))?;
            // Resume requires the pausing run or an explicit takeover.
            let is_owner = current_issue.changed_by_run_id.as_deref() == Some(run.run_id.as_str());
            if !is_owner && !request.takeover {
                return Err(run_conflict(format!(
                    "ISSUE-{issue_number:03} was paused by AgentRun {}; resume it with that run or pass takeover=true",
                    current_issue.changed_by_run_id.as_deref().unwrap_or("?")
                )));
            }
            // Resuming must not create a second In Progress Issue for the same session.
            if let Some(session_key) = run.host_session_id.as_deref() {
                let held_issue: Option<i64> = sqlx::query_scalar(
                    "SELECT n.issue_number
                     FROM agent_runs r
                     JOIN native_issues n
                       ON n.project_id = r.project_id AND n.issue_number = r.issue_number
                     WHERE r.project_id = $1
                       AND r.host_session_id = $3
                       AND r.phase = 'running'
                       AND r.lease_expires_at > $4
                       AND n.status = 'in_progress'
                     ORDER BY n.issue_number LIMIT 1",
                )
                .bind(&request.project_id)
                .bind(&run.run_id)
                .bind(session_key)
                .bind(&now)
                .fetch_optional(&mut *tx)
                .await?;
                if let Some(held_issue) = held_issue {
                    return Err(run_conflict(format!(
                        "this session is already working on ISSUE-{held_issue:03} (In Progress); pause it before resuming ISSUE-{issue_number:03}"
                    )));
                }
            }
            Some(run)
        }
        None => {
            // Human (desktop) path: resume without an AgentRun. Refuse while
            // another AgentRun is still actively working this Issue.
            let active_runs: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM agent_runs
                 WHERE project_id = $1 AND issue_number = $2
                   AND phase = 'running' AND lease_expires_at > $3",
            )
            .bind(&request.project_id)
            .bind(issue_number)
            .bind(&now)
            .fetch_one(&mut *tx)
            .await?;
            if active_runs > 0 {
                return Err(run_conflict(format!(
                    "ISSUE-{issue_number:03} is being worked on by {active_runs} active AgentRun(s); resume it from that session"
                )));
            }
            // Unbind stale (ended) run bindings left on this Issue so the
            // board reflects that no AgentRun holds it anymore.
            sqlx::query(
                "UPDATE agent_runs
                 SET issue_number = NULL, revision = revision + 1, last_seen_at = $3
                 WHERE project_id = $1 AND issue_number = $2
                   AND NOT (phase = 'running' AND lease_expires_at > $3)",
            )
            .bind(&request.project_id)
            .bind(issue_number)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
            None
        }
    };
    let issue = set_native_issue_state_tx(
        &mut tx,
        &request.project_id,
        issue_number,
        IssueBoardState::InProgress,
        run.as_ref().map(|run| run.run_id.as_str()),
        None,
        &now,
    )
    .await?;
    tx.commit().await?;
    Ok(issue_workflow_mutation_response(run, &issue))
}

pub(crate) async fn create_issue(
    pool: &SqlitePool,
    request: CreateIssueRequest,
) -> Result<IssueMutationResponse, DaemonError> {
    validate_issue_create(&request)?;
    let external_references = normalize_issue_external_references(&request.external_references)?;
    let dependencies = parse_dependencies(&request.project_id, &request.dependencies)?;
    let blocking_facts = normalize_issue_blocking_facts(&request.blocking_facts)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let next_number: Option<i64> = sqlx::query_scalar(
        "WITH RECURSIVE candidates(issue_number) AS (
            SELECT 1
            UNION ALL
            SELECT issue_number + 1 FROM candidates WHERE issue_number < 999
         )
         SELECT MIN(candidates.issue_number)
         FROM candidates
         LEFT JOIN native_issues
           ON native_issues.project_id = $1
          AND native_issues.issue_number = candidates.issue_number
         WHERE native_issues.issue_number IS NULL",
    )
    .bind(&request.project_id)
    .fetch_one(&mut *tx)
    .await?;
    let next_number = next_number.ok_or_else(|| {
        DaemonError::InvalidRequest("Issue numbers 001 through 999 are already in use".to_owned())
    })?;
    let (now, _) = db_clock(&mut tx).await?;
    let issue_id = format!("issue_{}", Uuid::new_v4().simple());
    let criteria_json = serde_json::to_string(&request.acceptance_criteria)?;
    let external_references_json = serde_json::to_string(&external_references)?;
    let verification_steps_json = serde_json::to_string(&request.verification_steps)?;
    sqlx::query(
        "INSERT INTO native_issues (
            issue_id, project_id, issue_number, title, description,
            acceptance_criteria_json, external_references_json,
            status, revision, changed_by_run_id, closure_summary,
            verification_level, verification_steps_json,
            created_at, started_at, updated_at, closed_at, archived_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7,
                   'todo', 1, NULL, NULL, $8, $9,
                   $10, NULL, $10, NULL, NULL)",
    )
    .bind(&issue_id)
    .bind(&request.project_id)
    .bind(next_number)
    .bind(request.title.trim())
    .bind(request.description.trim())
    .bind(criteria_json)
    .bind(external_references_json)
    .bind(request.verification_level.as_str())
    .bind(verification_steps_json)
    .bind(&now)
    .execute(&mut *tx)
    .await?;
    validate_dependencies_exist_tx(
        &mut tx,
        &request.project_id,
        &dependencies,
        Some(next_number),
    )
    .await?;
    if !dependencies.is_empty() {
        assert_dependency_graph_acyclic_tx(
            &mut tx,
            &request.project_id,
            next_number,
            &dependencies,
        )
        .await?;
        insert_dependencies_tx(
            &mut tx,
            &request.project_id,
            next_number,
            &dependencies,
            &now,
        )
        .await?;
    }
    if !blocking_facts.is_empty() {
        insert_blocking_facts_tx(
            &mut tx,
            &request.project_id,
            next_number,
            &blocking_facts,
            &now,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(IssueMutationResponse {
        issue_id,
        issue_key: format!("ISSUE-{next_number:03}"),
        board_state: IssueBoardState::Todo,
        revision: 1,
        updated_at: now,
    })
}

pub(crate) async fn update_issue(
    pool: &SqlitePool,
    request: UpdateIssueRequest,
) -> Result<IssueMutationResponse, DaemonError> {
    validate_issue_update(&request)?;
    let normalized_external_references = request
        .external_references
        .as_deref()
        .map(normalize_issue_external_references)
        .transpose()?;
    let dependencies = request
        .dependencies
        .as_deref()
        .map(|keys| parse_dependencies(&request.project_id, keys))
        .transpose()?;
    let blocking_facts = request
        .blocking_facts
        .as_deref()
        .map(normalize_issue_blocking_facts)
        .transpose()?;
    let issue_number = parse_issue_reference(&request.issue_key)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let current = load_native_issue_tx(&mut tx, &request.project_id, issue_number)
        .await?
        .ok_or_else(|| DaemonError::NotFound(format!("ISSUE-{issue_number:03}")))?;
    ensure_issue_revision(&current, request.expected_revision)?;
    let title = request
        .title
        .as_deref()
        .map(str::trim)
        .unwrap_or(&current.title);
    let description = request
        .description
        .as_deref()
        .map(str::trim)
        .unwrap_or(&current.description);
    let criteria = request
        .acceptance_criteria
        .as_ref()
        .unwrap_or(&current.acceptance_criteria);
    let external_references = normalized_external_references
        .as_ref()
        .unwrap_or(&current.external_references);
    let dependencies = dependencies.unwrap_or_else(|| current.dependencies.clone());
    let blocking_facts = blocking_facts.unwrap_or_else(|| current.blocking_facts.clone());
    let verification_level = request
        .verification_level
        .unwrap_or(current.verification_level);
    let verification_steps = request
        .verification_steps
        .as_ref()
        .unwrap_or(&current.verification_steps);
    let criteria_json = serde_json::to_string(criteria)?;
    let external_references_json = serde_json::to_string(external_references)?;
    let verification_steps_json = serde_json::to_string(verification_steps)?;
    let (now, _) = db_clock(&mut tx).await?;
    let revision = current.revision + 1;
    sqlx::query(
        "UPDATE native_issues
         SET title = $3, description = $4, acceptance_criteria_json = $5,
             external_references_json = $6, revision = $7, updated_at = $8,
             verification_level = $9, verification_steps_json = $10
         WHERE project_id = $1 AND issue_number = $2",
    )
    .bind(&request.project_id)
    .bind(issue_number)
    .bind(title)
    .bind(description)
    .bind(criteria_json)
    .bind(external_references_json)
    .bind(revision)
    .bind(&now)
    .bind(verification_level.as_str())
    .bind(verification_steps_json)
    .execute(&mut *tx)
    .await?;
    if request.dependencies.is_some() {
        validate_dependencies_exist_tx(
            &mut tx,
            &request.project_id,
            &dependencies,
            Some(issue_number),
        )
        .await?;
        assert_dependency_graph_acyclic_tx(
            &mut tx,
            &request.project_id,
            issue_number,
            &dependencies,
        )
        .await?;
        sqlx::query(
            "DELETE FROM issue_dependencies
             WHERE project_id = $1 AND issue_number = $2",
        )
        .bind(&request.project_id)
        .bind(issue_number)
        .execute(&mut *tx)
        .await?;
        insert_dependencies_tx(
            &mut tx,
            &request.project_id,
            issue_number,
            &dependencies,
            &now,
        )
        .await?;
    }
    if request.blocking_facts.is_some() {
        sqlx::query(
            "DELETE FROM issue_blocking_facts
             WHERE project_id = $1 AND issue_number = $2",
        )
        .bind(&request.project_id)
        .bind(issue_number)
        .execute(&mut *tx)
        .await?;
        insert_blocking_facts_tx(
            &mut tx,
            &request.project_id,
            issue_number,
            &blocking_facts,
            &now,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(IssueMutationResponse {
        issue_id: current.issue_id,
        issue_key: format!("ISSUE-{issue_number:03}"),
        board_state: current.board_state,
        revision,
        updated_at: now,
    })
}

pub(crate) async fn apply_issue_gate(
    pool: &SqlitePool,
    request: ApplyIssueGateRequest,
) -> Result<IssueMutationResponse, DaemonError> {
    validate_required("project_id", &request.project_id, MAX_IDENTIFIER_BYTES)?;
    if !(1..=999).contains(&request.issue_number) {
        return Err(DaemonError::InvalidRequest(
            "issue_number must be between 1 and 999".to_owned(),
        ));
    }
    validate_revision(request.expected_revision)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let current = load_native_issue_tx(&mut tx, &request.project_id, request.issue_number)
        .await?
        .ok_or_else(|| DaemonError::NotFound(format!("ISSUE-{:03}", request.issue_number)))?;
    ensure_issue_revision(&current, request.expected_revision)?;
    if request.action == IssueGateAction::ApproveClosure {
        let pending = current
            .verification_steps
            .iter()
            .enumerate()
            .filter_map(|(index, step)| (!step.completed).then_some(index))
            .collect::<Vec<_>>();
        if !pending.is_empty() {
            let list = pending
                .iter()
                .map(|index| format!("step {}", index + 1))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(DaemonError::InvalidRequest(format!(
                "cannot approve ISSUE-{:03}: verification {} still incomplete: {list}",
                request.issue_number,
                if pending.len() == 1 {
                    "step is"
                } else {
                    "steps are"
                }
            )));
        }
    }
    let target = match (request.action, current.board_state) {
        (IssueGateAction::ApproveClosure, IssueBoardState::InReview) => IssueBoardState::Done,
        (IssueGateAction::RequestChanges, IssueBoardState::InReview) => IssueBoardState::InProgress,
        (IssueGateAction::Reopen, IssueBoardState::Done) => IssueBoardState::Todo,
        (action, state) => {
            return Err(run_conflict(format!(
                "gate {action:?} is not valid while ISSUE-{:03} is {state:?}",
                request.issue_number
            )));
        }
    };
    let (now, _) = db_clock(&mut tx).await?;
    let revision = current.revision + 1;
    let closed_at = (target == IssueBoardState::Done).then_some(now.as_str());
    let started_at = (request.action != IssueGateAction::Reopen)
        .then_some(current.started_at.as_deref())
        .flatten();
    let closure_summary = (request.action == IssueGateAction::ApproveClosure)
        .then_some(current.closure_summary.as_deref())
        .flatten();
    sqlx::query(
        "UPDATE native_issues
         SET status = $3, revision = $4, changed_by_run_id = NULL,
             closure_summary = $5, updated_at = $6, started_at = $7,
             closed_at = $8
         WHERE project_id = $1 AND issue_number = $2",
    )
    .bind(&request.project_id)
    .bind(request.issue_number)
    .bind(target.as_str())
    .bind(revision)
    .bind(closure_summary)
    .bind(&now)
    .bind(started_at)
    .bind(closed_at)
    .execute(&mut *tx)
    .await?;
    record_issue_state_event_tx(
        &mut tx,
        &request.project_id,
        request.issue_number,
        current.board_state,
        target,
        None,
        &now,
    )
    .await?;
    tx.commit().await?;
    Ok(IssueMutationResponse {
        issue_id: current.issue_id,
        issue_key: format!("ISSUE-{:03}", request.issue_number),
        board_state: target,
        revision,
        updated_at: now,
    })
}

pub(crate) async fn unclaim_issue(
    pool: &SqlitePool,
    request: UnclaimIssueRequest,
) -> Result<IssueMutationResponse, DaemonError> {
    validate_required("project_id", &request.project_id, MAX_IDENTIFIER_BYTES)?;
    if let Some(run_id) = request.run_id.as_deref() {
        validate_required("run_id", run_id, MAX_IDENTIFIER_BYTES)?;
    }
    let issue_number = parse_issue_reference(&request.issue_key)?;
    validate_revision(request.expected_revision)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let current = load_native_issue_tx(&mut tx, &request.project_id, issue_number)
        .await?
        .ok_or_else(|| DaemonError::NotFound(format!("ISSUE-{issue_number:03}")))?;
    ensure_issue_revision(&current, request.expected_revision)?;
    if !matches!(
        current.board_state,
        IssueBoardState::InProgress | IssueBoardState::Paused
    ) {
        return Err(run_conflict(format!(
            "ISSUE-{issue_number:03} is {:?}; only In Progress or Paused Issues can be released",
            current.board_state
        )));
    }
    let (now, _) = db_clock(&mut tx).await?;
    if let Some(run_id) = request.run_id.as_deref() {
        // Agent path: the releasing run must exist and be bound to this Issue.
        let run = load_agent_run_for_project_tx(&mut tx, &request.project_id, run_id)
            .await?
            .ok_or_else(|| DaemonError::NotFound(format!("AgentRun {run_id}")))?;
        if run.issue_number != Some(issue_number) {
            return Err(run_conflict(format!(
                "AgentRun {} is not bound to ISSUE-{issue_number:03}",
                run.run_id
            )));
        }
        let mut run = run;
        run.issue_number = None;
        run.revision += 1;
        run.last_seen_at = now.clone();
        update_run(&mut tx, &run).await?;
    } else {
        // Human (desktop) path: release without an AgentRun. Refuse while any
        // AgentRun is still actively working this Issue so a human cannot
        // yank work out from under a live session.
        let active_runs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM agent_runs
             WHERE project_id = $1 AND issue_number = $2
               AND phase = 'running' AND lease_expires_at > $3",
        )
        .bind(&request.project_id)
        .bind(issue_number)
        .bind(&now)
        .fetch_one(&mut *tx)
        .await?;
        if active_runs > 0 {
            return Err(run_conflict(format!(
                "ISSUE-{issue_number:03} is being worked on by {active_runs} active AgentRun(s); release it from that session"
            )));
        }
        // Unbind stale (ended) run bindings left on this Issue.
        sqlx::query(
            "UPDATE agent_runs
             SET issue_number = NULL, revision = revision + 1, last_seen_at = $3
             WHERE project_id = $1 AND issue_number = $2
               AND NOT (phase = 'running' AND lease_expires_at > $3)",
        )
        .bind(&request.project_id)
        .bind(issue_number)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    }
    let revision = current.revision + 1;
    sqlx::query(
        "UPDATE native_issues
         SET status = 'todo', revision = $3, changed_by_run_id = NULL,
             updated_at = $4
         WHERE project_id = $1 AND issue_number = $2",
    )
    .bind(&request.project_id)
    .bind(issue_number)
    .bind(revision)
    .bind(&now)
    .execute(&mut *tx)
    .await?;
    record_issue_state_event_tx(
        &mut tx,
        &request.project_id,
        issue_number,
        current.board_state,
        IssueBoardState::Todo,
        None,
        &now,
    )
    .await?;
    tx.commit().await?;
    Ok(IssueMutationResponse {
        issue_id: current.issue_id,
        issue_key: format!("ISSUE-{issue_number:03}"),
        board_state: IssueBoardState::Todo,
        revision,
        updated_at: now,
    })
}

pub(crate) async fn set_verification_step_completed(
    pool: &SqlitePool,
    request: SetVerificationStepCompletedRequest,
) -> Result<IssueMutationResponse, DaemonError> {
    validate_required("project_id", &request.project_id, MAX_IDENTIFIER_BYTES)?;
    let issue_number = parse_issue_reference(&request.issue_key)?;
    validate_revision(request.expected_revision)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let current = load_native_issue_tx(&mut tx, &request.project_id, issue_number)
        .await?
        .ok_or_else(|| DaemonError::NotFound(format!("ISSUE-{issue_number:03}")))?;
    ensure_issue_revision(&current, request.expected_revision)?;
    if current.board_state == IssueBoardState::Done {
        return Err(DaemonError::InvalidRequest(format!(
            "ISSUE-{issue_number:03} is Done; reopen it before editing verification steps"
        )));
    }
    let mut steps = current.verification_steps.clone();
    let step = steps.get_mut(request.step_index).ok_or_else(|| {
        DaemonError::InvalidRequest(format!(
            "verification step {} does not exist on ISSUE-{issue_number:03}",
            request.step_index
        ))
    })?;
    if step.completed == request.completed {
        tx.commit().await?;
        return Ok(IssueMutationResponse {
            issue_id: current.issue_id,
            issue_key: format!("ISSUE-{issue_number:03}"),
            board_state: current.board_state,
            revision: current.revision,
            updated_at: current.updated_at,
        });
    }
    step.completed = request.completed;
    let steps_json = serde_json::to_string(&steps)?;
    let revision = current.revision + 1;
    let (now, _) = db_clock(&mut tx).await?;
    sqlx::query(
        "UPDATE native_issues
         SET verification_steps_json = $3, revision = $4, updated_at = $5
         WHERE project_id = $1 AND issue_number = $2",
    )
    .bind(&request.project_id)
    .bind(issue_number)
    .bind(steps_json)
    .bind(revision)
    .bind(&now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(IssueMutationResponse {
        issue_id: current.issue_id,
        issue_key: format!("ISSUE-{issue_number:03}"),
        board_state: current.board_state,
        revision,
        updated_at: now,
    })
}

pub(crate) async fn remove_issue(
    pool: &SqlitePool,
    request: RemoveIssueRequest,
) -> Result<IssueRemovalResponse, DaemonError> {
    validate_required("project_id", &request.project_id, MAX_IDENTIFIER_BYTES)?;
    if !(1..=999).contains(&request.issue_number) {
        return Err(DaemonError::InvalidRequest(
            "issue_number must be between 1 and 999".to_owned(),
        ));
    }
    validate_revision(request.expected_revision)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let current = load_native_issue_tx(&mut tx, &request.project_id, request.issue_number)
        .await?
        .ok_or_else(|| DaemonError::NotFound(format!("ISSUE-{:03}", request.issue_number)))?;
    ensure_issue_revision(&current, request.expected_revision)?;
    let (now, _) = db_clock(&mut tx).await?;
    match request.action {
        IssueRemovalAction::Archive => {
            if current.board_state != IssueBoardState::Done {
                return Err(run_conflict(format!(
                    "ISSUE-{:03} must be Done before it can be archived",
                    request.issue_number
                )));
            }
            sqlx::query(
                "UPDATE native_issues
                 SET archived_at = $3, updated_at = $3, revision = revision + 1
                 WHERE project_id = $1 AND issue_number = $2",
            )
            .bind(&request.project_id)
            .bind(request.issue_number)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }
        IssueRemovalAction::Delete => {
            if current.board_state == IssueBoardState::Done {
                return Err(run_conflict(format!(
                    "ISSUE-{:03} is Done and must be archived instead of deleted",
                    request.issue_number
                )));
            }
            sqlx::query(
                "UPDATE agent_runs SET issue_number = NULL
                 WHERE project_id = $1 AND issue_number = $2",
            )
            .bind(&request.project_id)
            .bind(request.issue_number)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "DELETE FROM issue_dependencies
                 WHERE project_id = $1 AND issue_number = $2",
            )
            .bind(&request.project_id)
            .bind(request.issue_number)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "DELETE FROM issue_dependencies
                 WHERE project_id = $1 AND depends_on_number = $2",
            )
            .bind(&request.project_id)
            .bind(request.issue_number)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "DELETE FROM issue_blocking_facts
                 WHERE project_id = $1 AND issue_number = $2",
            )
            .bind(&request.project_id)
            .bind(request.issue_number)
            .execute(&mut *tx)
            .await?;
            sqlx::query("DELETE FROM native_issues WHERE project_id = $1 AND issue_number = $2")
                .bind(&request.project_id)
                .bind(request.issue_number)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(IssueRemovalResponse {
        issue_id: current.issue_id,
        issue_key: format!("ISSUE-{:03}", request.issue_number),
        action: request.action,
        removed_at: now,
    })
}

pub(crate) async fn native_issue_import_completed(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<bool, DaemonError> {
    let imported: i64 = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM native_issue_imports WHERE project_id = $1
         )",
    )
    .bind(project_id)
    .fetch_one(pool)
    .await?;
    Ok(imported != 0)
}

pub(crate) async fn import_legacy_issues(
    pool: &SqlitePool,
    project_id: &str,
    cards: &[IssueBoardCard],
    resources: &[SourceResource],
) -> Result<(), DaemonError> {
    validate_required("project_id", project_id, MAX_IDENTIFIER_BYTES)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let already_imported: i64 = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM native_issue_imports WHERE project_id = $1
         )",
    )
    .bind(project_id)
    .fetch_one(&mut *tx)
    .await?;
    if already_imported != 0 {
        tx.commit().await?;
        return Ok(());
    }
    let (now, _) = db_clock(&mut tx).await?;
    for card in cards {
        let legacy_content = resources
            .iter()
            .find(|resource| resource.resource_id == card.resource_id && resource.path == card.path)
            .map(|resource| resource.content.trim())
            .unwrap_or("");
        let (description, acceptance_criteria) = parse_legacy_issue_content(legacy_content);
        let criteria_json = serde_json::to_string(&acceptance_criteria)?;
        sqlx::query(
            "INSERT OR IGNORE INTO native_issues (
                issue_id, project_id, issue_number, title, description,
                acceptance_criteria_json,
                status, revision, changed_by_run_id, closure_summary,
                created_at, started_at, updated_at, closed_at, archived_at
             ) VALUES ($1, $2, $3, $4, $5, $6,
                       $7, 1, NULL, $8, $9, NULL, $9, $10, NULL)",
        )
        .bind(format!("issue_{}", Uuid::new_v4().simple()))
        .bind(project_id)
        .bind(card.issue_number)
        .bind(&card.title)
        .bind(description)
        .bind(criteria_json)
        .bind(card.board_state.as_str())
        .bind(&card.closure_summary)
        .bind(&now)
        .bind((card.board_state == IssueBoardState::Done).then_some(now.as_str()))
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO native_issue_imports (project_id, imported_at)
         VALUES ($1, $2)",
    )
    .bind(project_id)
    .bind(&now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn project_native_issue_board(
    pool: &SqlitePool,
    project_id: &str,
    runs: &[AgentRun],
    now: &str,
    stale_before: &str,
) -> Result<Vec<IssueBoardCard>, DaemonError> {
    let issues = load_native_issues(pool, project_id).await?;
    let states = load_native_issue_states(pool, project_id).await?;
    let mut cards = Vec::with_capacity(issues.len());
    for issue in issues {
        let issue_runs = runs
            .iter()
            .filter(|run| {
                run.project_id == project_id && run.issue_number == Some(issue.issue_number)
            })
            .cloned()
            .collect::<Vec<_>>();
        let projection = project_runs(&issue_runs, now);
        cards.push(native_issue_card(issue, projection, stale_before, &states));
    }
    Ok(cards)
}

pub(crate) async fn load_native_issue_detail(
    pool: &SqlitePool,
    project_id: &str,
    issue_number: i64,
    runs: &[AgentRun],
    now: &str,
    stale_before: &str,
) -> Result<IssueDetailResponse, DaemonError> {
    let mut tx = pool.begin().await?;
    let issue = load_native_issue_tx(&mut tx, project_id, issue_number)
        .await?
        .ok_or_else(|| DaemonError::NotFound(format!("ISSUE-{issue_number:03}")))?;
    tx.commit().await?;
    let issue_runs = runs
        .iter()
        .filter(|run| run.project_id == project_id && run.issue_number == Some(issue_number))
        .cloned()
        .collect::<Vec<_>>();
    let projection = project_runs(&issue_runs, now);
    let states = load_native_issue_states(pool, project_id).await?;
    let body = issue.description.clone();
    let acceptance_criteria = issue.acceptance_criteria.clone();
    Ok(IssueDetailResponse {
        issue: native_issue_card(issue, projection, stale_before, &states),
        body,
        acceptance_criteria,
    })
}

pub(crate) async fn resolve_native_issue_identity(
    pool: &SqlitePool,
    issue_id: &str,
) -> Result<Option<(String, i64)>, DaemonError> {
    validate_issue_id(issue_id)?;
    sqlx::query_as("SELECT project_id, issue_number FROM native_issues WHERE issue_id = $1")
        .bind(issue_id)
        .fetch_optional(pool)
        .await
        .map_err(Into::into)
}

/// Confirms an Issue with the given number exists in the given project.
/// Returns the issue_number when found, or None when the project has no such Issue.
pub(crate) async fn ensure_native_issue_in_project(
    pool: &SqlitePool,
    project_id: &str,
    issue_number: i64,
) -> Result<Option<i64>, DaemonError> {
    let found: Option<i64> = sqlx::query_scalar(
        "SELECT issue_number FROM native_issues
         WHERE project_id = $1 AND issue_number = $2",
    )
    .bind(project_id)
    .bind(issue_number)
    .fetch_optional(pool)
    .await?;
    Ok(found)
}

pub(crate) async fn native_issue_id(
    pool: &SqlitePool,
    project_id: &str,
    issue_number: i64,
) -> Result<Option<String>, DaemonError> {
    sqlx::query_scalar(
        "SELECT issue_id FROM native_issues
         WHERE project_id = $1 AND issue_number = $2",
    )
    .bind(project_id)
    .bind(issue_number)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub(crate) async fn native_issue_acceptance_criteria(
    pool: &SqlitePool,
    project_id: &str,
    issue_number: i64,
) -> Result<Vec<String>, DaemonError> {
    let value: String = sqlx::query_scalar(
        "SELECT acceptance_criteria_json FROM native_issues
         WHERE project_id = $1 AND issue_number = $2",
    )
    .bind(project_id)
    .bind(issue_number)
    .fetch_one(pool)
    .await?;
    serde_json::from_str(&value)
        .map_err(|error| corrupt_run(format!("invalid Issue acceptance criteria: {error}")))
}

pub(crate) async fn sync_server_kanban_issues(
    pool: &SqlitePool,
    project_id: &str,
    records: &[ServerKanbanIssue],
) -> Result<(), DaemonError> {
    if records.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let (now, _) = db_clock(&mut tx).await?;
    let local_run_ids =
        sqlx::query_scalar::<_, String>("SELECT run_id FROM agent_runs WHERE project_id = $1")
            .bind(project_id)
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .collect::<BTreeSet<_>>();
    for record in records {
        let issue = &record.payload.issue;
        if record.project_id != project_id
            || issue.project_id != project_id
            || record.issue_id != issue.issue_id
            || record.issue_number != issue.issue_number
            || record.content_revision != issue.state_revision
        {
            return Err(corrupt_run(
                "Server Kanban Issue identity mismatch".to_owned(),
            ));
        }
        sqlx::query(
            "INSERT INTO native_issues (
                issue_id, project_id, issue_number, title, description,
                acceptance_criteria_json, external_references_json,
                status, revision, changed_by_run_id, closure_summary,
                verification_level, verification_steps_json,
                created_at, started_at, updated_at, closed_at, archived_at
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                       $12, $13, $14, $15, $16, $17, $18)
             ON CONFLICT(project_id, issue_number) DO UPDATE SET
                issue_id = excluded.issue_id,
                title = excluded.title,
                description = excluded.description,
                acceptance_criteria_json = excluded.acceptance_criteria_json,
                external_references_json = excluded.external_references_json,
                status = excluded.status,
                revision = excluded.revision,
                changed_by_run_id = excluded.changed_by_run_id,
                closure_summary = excluded.closure_summary,
                verification_level = excluded.verification_level,
                verification_steps_json = excluded.verification_steps_json,
                created_at = excluded.created_at,
                started_at = excluded.started_at,
                updated_at = excluded.updated_at,
                closed_at = excluded.closed_at,
                archived_at = excluded.archived_at",
        )
        .bind(&issue.issue_id)
        .bind(project_id)
        .bind(issue.issue_number)
        .bind(&issue.title)
        .bind(&issue.description)
        .bind(serde_json::to_string(&record.payload.acceptance_criteria)?)
        .bind(serde_json::to_string(&issue.external_references)?)
        .bind(issue.board_state.as_str())
        .bind(issue.state_revision)
        .bind(
            issue
                .changed_by_run_id
                .as_ref()
                .filter(|run_id| local_run_ids.contains(*run_id)),
        )
        .bind(&issue.closure_summary)
        .bind(issue.verification_level.as_str())
        .bind(serde_json::to_string(&issue.verification_steps)?)
        .bind(
            issue
                .created_at
                .as_deref()
                .or(issue.found_at.as_deref())
                .unwrap_or(&now),
        )
        .bind(&issue.started_at)
        .bind(issue.state_updated_at.as_deref().unwrap_or(&now))
        .bind(&issue.closed_at)
        .bind(&issue.archived_at)
        .execute(&mut *tx)
        .await?;
    }

    for statement in [
        "DELETE FROM issue_dependencies WHERE project_id = $1",
        "DELETE FROM issue_blocking_facts WHERE project_id = $1",
        "DELETE FROM issue_state_events WHERE project_id = $1",
    ] {
        sqlx::query(statement)
            .bind(project_id)
            .execute(&mut *tx)
            .await?;
    }
    for record in records {
        let issue = &record.payload.issue;
        for dependency in &issue.dependencies {
            let depends_on_number = parse_issue_reference(&dependency.issue_key)?;
            sqlx::query(
                "INSERT INTO issue_dependencies (
                    project_id, issue_number, depends_on_number, created_at
                 ) VALUES ($1, $2, $3, $4)",
            )
            .bind(project_id)
            .bind(issue.issue_number)
            .bind(depends_on_number)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }
        for fact in &issue.blocking_facts {
            sqlx::query(
                "INSERT INTO issue_blocking_facts (
                    project_id, issue_number, fact_id, kind, value,
                    description, satisfied, created_at, updated_at
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)",
            )
            .bind(project_id)
            .bind(issue.issue_number)
            .bind(&fact.fact_id)
            .bind(fact.kind.as_str())
            .bind(&fact.value)
            .bind(&fact.description)
            .bind(if fact.satisfied { 1_i64 } else { 0_i64 })
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }
        for event in &issue.state_events {
            sqlx::query(
                "INSERT INTO issue_state_events (
                    event_id, project_id, issue_number, from_state, to_state,
                    changed_by_run_id, occurred_at
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(format!("evt_{}", Uuid::new_v4().simple()))
            .bind(project_id)
            .bind(issue.issue_number)
            .bind(event.from_state.as_str())
            .bind(event.to_state.as_str())
            .bind(
                event
                    .changed_by_run_id
                    .as_ref()
                    .filter(|run_id| local_run_ids.contains(*run_id)),
            )
            .bind(&event.occurred_at)
            .execute(&mut *tx)
            .await?;
        }
    }
    sqlx::query(
        "INSERT INTO native_issue_imports (project_id, imported_at)
         VALUES ($1, $2)
         ON CONFLICT(project_id) DO UPDATE SET imported_at = excluded.imported_at",
    )
    .bind(project_id)
    .bind(&now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) fn native_board_hash(cards: &[IssueBoardCard]) -> String {
    let mut hasher = Sha256::new();
    for card in cards {
        hasher.update(card.issue_key.as_bytes());
        hasher.update(card.state_revision.to_be_bytes());
        if let Some(updated_at) = &card.state_updated_at {
            hasher.update(updated_at.as_bytes());
        }
    }
    format!("{:x}", hasher.finalize())
}

/// Builds a short readable excerpt from an Issue description: takes the first
/// non-empty paragraph, strips Markdown heading/list markers, and caps the
/// length at a few hundred characters.
fn issue_description_excerpt(description: &str) -> String {
    const MAX_EXCERPT_BYTES: usize = 240;

    let first_paragraph = description
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            let without_marker = trimmed
                .strip_prefix('-')
                .or_else(|| trimmed.strip_prefix('*'))
                .or_else(|| trimmed.strip_prefix('>'))
                .unwrap_or(trimmed);
            let cleaned = without_marker.trim();
            (!cleaned.is_empty()).then_some(cleaned)
        })
        .next()
        .unwrap_or("");

    if first_paragraph.is_empty() {
        return String::new();
    }
    if first_paragraph.len() <= MAX_EXCERPT_BYTES {
        return first_paragraph.to_owned();
    }
    // Ellipsis is 3 UTF-8 bytes; leave room so the result stays within budget.
    let mut end = MAX_EXCERPT_BYTES - 4;
    let max_prefix = 64;
    while end > max_prefix && !first_paragraph.is_char_boundary(end) {
        end -= 1;
    }
    let mut excerpt = first_paragraph[..end].trim_end().to_owned();
    if excerpt.len() < first_paragraph.len() {
        excerpt.push('…');
    }
    excerpt
}

fn native_issue_card(
    issue: NativeIssue,
    projection: IssueRunProjection,
    stale_before: &str,
    states: &BTreeMap<i64, (String, IssueBoardState)>,
) -> IssueBoardCard {
    let description_excerpt = issue_description_excerpt(&issue.description);
    let latest_activity = projection
        .latest_run
        .as_ref()
        .map(meaningful_timestamp)
        .unwrap_or(issue.updated_at.as_str());
    let is_stale = issue.board_state == IssueBoardState::InProgress
        && projection.active_runs.is_empty()
        && latest_activity <= stale_before;
    let dependencies = issue
        .dependencies
        .iter()
        .map(|depends_on_number| match states.get(depends_on_number) {
            Some((title, board_state)) => IssueDependencyState {
                issue_key: format!("ISSUE-{depends_on_number:03}"),
                title: title.clone(),
                board_state: *board_state,
            },
            None => IssueDependencyState {
                issue_key: format!("ISSUE-{depends_on_number:03}"),
                title: "(missing)".to_owned(),
                board_state: IssueBoardState::Todo,
            },
        })
        .collect::<Vec<_>>();
    let mut blocking_reasons = Vec::new();
    for dependency in &dependencies {
        if dependency.board_state != IssueBoardState::Done {
            blocking_reasons.push(IssueBlockingReason {
                kind: IssueBlockingReasonKind::Dependency,
                issue_key: Some(dependency.issue_key.clone()),
                title: Some(dependency.title.clone()),
                board_state: Some(dependency.board_state),
                fact_id: None,
                description: None,
            });
        }
    }
    for fact in issue.blocking_facts.iter().filter(|fact| !fact.satisfied) {
        blocking_reasons.push(IssueBlockingReason {
            kind: IssueBlockingReasonKind::Fact,
            issue_key: None,
            title: None,
            board_state: None,
            fact_id: Some(fact.fact_id.clone()),
            description: Some(fact.description.clone()),
        });
    }
    let blocked = !blocking_reasons.is_empty();
    IssueBoardCard {
        issue_id: issue.issue_id.clone(),
        project_id: issue.project_id,
        issue_number: issue.issue_number,
        issue_key: format!("ISSUE-{:03}", issue.issue_number),
        resource_id: issue.issue_id,
        path: String::new(),
        lifecycle: if issue.board_state == IssueBoardState::Done {
            IssueLifecycle::Closed
        } else {
            IssueLifecycle::Open
        },
        title: issue.title,
        description: issue.description,
        description_excerpt,
        external_references: issue.external_references,
        found_at: Some(issue.created_at.clone()),
        created_at: Some(issue.created_at),
        started_at: issue.started_at,
        closed_at: issue.closed_at,
        archived_at: issue.archived_at,
        content_hash: format!("native:{}", issue.revision),
        source_commit_id: None,
        draft_id: None,
        draft_revision: None,
        board_state: issue.board_state,
        state_revision: issue.revision,
        state_updated_at: Some(issue.updated_at),
        closure_summary: issue.closure_summary,
        assignee: None,
        is_stale,
        blocked,
        blocking_reasons,
        dependencies,
        blocking_facts: issue.blocking_facts,
        active_runs: projection.active_runs,
        latest_run: projection.latest_run,
        changed_by_run_id: issue.changed_by_run_id.clone(),
        verification_level: issue.verification_level,
        verification_steps: issue.verification_steps,
        state_events: issue.state_events,
    }
}

async fn load_native_issues(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<Vec<NativeIssue>, DaemonError> {
    let rows = sqlx::query(
        "SELECT issue_id, project_id, issue_number, title, description,
                acceptance_criteria_json, external_references_json,
                status, revision, changed_by_run_id, closure_summary,
                verification_level, verification_steps_json,
                created_at, started_at, updated_at, closed_at, archived_at
         FROM native_issues
         WHERE project_id = $1 AND archived_at IS NULL
         ORDER BY issue_number",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    let dependencies = load_project_dependencies(pool, project_id).await?;
    let blocking_facts = load_project_blocking_facts(pool, project_id).await?;
    let state_events = load_project_state_events(pool, project_id).await?;
    rows.iter()
        .map(|row| {
            native_issue_from_row(row).map(|mut issue| {
                issue.dependencies = dependencies
                    .get(&issue.issue_number)
                    .cloned()
                    .unwrap_or_default();
                issue.blocking_facts = blocking_facts
                    .get(&issue.issue_number)
                    .cloned()
                    .unwrap_or_default();
                issue.state_events = state_events
                    .get(&issue.issue_number)
                    .cloned()
                    .unwrap_or_default();
                issue
            })
        })
        .collect()
}

async fn load_project_state_events(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<BTreeMap<i64, Vec<IssueStateEvent>>, DaemonError> {
    let rows = sqlx::query(
        "SELECT issue_number, from_state, to_state, changed_by_run_id, occurred_at
         FROM issue_state_events
         WHERE project_id = $1
         ORDER BY occurred_at ASC, event_id ASC",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    let mut events: BTreeMap<i64, Vec<IssueStateEvent>> = BTreeMap::new();
    for row in rows {
        let issue_number: i64 = row.try_get("issue_number")?;
        let from_state: String = row.try_get("from_state")?;
        let to_state: String = row.try_get("to_state")?;
        events
            .entry(issue_number)
            .or_default()
            .push(IssueStateEvent {
                from_state: IssueBoardState::from_db(&from_state)?,
                to_state: IssueBoardState::from_db(&to_state)?,
                changed_by_run_id: row.try_get("changed_by_run_id")?,
                occurred_at: row.try_get("occurred_at")?,
            });
    }
    Ok(events)
}

/// All native Issue numbers, titles, and board states for a Project, including
/// archived Issues, so dependency resolution does not depend on board visibility.
async fn load_native_issue_states(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<BTreeMap<i64, (String, IssueBoardState)>, DaemonError> {
    let rows =
        sqlx::query("SELECT issue_number, title, status FROM native_issues WHERE project_id = $1")
            .bind(project_id)
            .fetch_all(pool)
            .await?;
    let mut states = BTreeMap::new();
    for row in rows {
        let issue_number: i64 = row.try_get("issue_number")?;
        let title: String = row.try_get("title")?;
        let status: String = row.try_get("status")?;
        states.insert(issue_number, (title, IssueBoardState::from_db(&status)?));
    }
    Ok(states)
}

async fn load_project_dependencies(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<BTreeMap<i64, Vec<i64>>, DaemonError> {
    let rows = sqlx::query(
        "SELECT issue_number, depends_on_number
         FROM issue_dependencies WHERE project_id = $1 ORDER BY depends_on_number",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    let mut dependencies = BTreeMap::<i64, Vec<i64>>::new();
    for row in rows {
        let issue_number: i64 = row.try_get("issue_number")?;
        let depends_on_number: i64 = row.try_get("depends_on_number")?;
        dependencies
            .entry(issue_number)
            .or_default()
            .push(depends_on_number);
    }
    Ok(dependencies)
}

async fn load_project_blocking_facts(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<BTreeMap<i64, Vec<IssueBlockingFact>>, DaemonError> {
    let rows = sqlx::query(
        "SELECT issue_number, fact_id, kind, value, description, satisfied
         FROM issue_blocking_facts
         WHERE project_id = $1 ORDER BY fact_id",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    let mut facts = BTreeMap::<i64, Vec<IssueBlockingFact>>::new();
    for row in rows {
        let issue_number: i64 = row.try_get("issue_number")?;
        let fact = blocking_fact_from_row(&row)?;
        facts.entry(issue_number).or_default().push(fact);
    }
    Ok(facts)
}

fn blocking_fact_from_row(row: &SqliteRow) -> Result<IssueBlockingFact, DaemonError> {
    let kind: String = row.try_get("kind")?;
    Ok(IssueBlockingFact {
        fact_id: row.try_get("fact_id")?,
        kind: IssueBlockingFactKind::from_db(&kind)?,
        value: row.try_get("value")?,
        description: row.try_get("description")?,
        satisfied: row.try_get::<i64, _>("satisfied")? != 0,
    })
}

async fn load_native_issue_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    issue_number: i64,
) -> Result<Option<NativeIssue>, DaemonError> {
    let row = sqlx::query(
        "SELECT issue_id, project_id, issue_number, title, description,
                acceptance_criteria_json, external_references_json,
                status, revision, changed_by_run_id, closure_summary,
                verification_level, verification_steps_json,
                created_at, started_at, updated_at, closed_at, archived_at
         FROM native_issues WHERE project_id = $1 AND issue_number = $2",
    )
    .bind(project_id)
    .bind(issue_number)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let mut issue = native_issue_from_row(&row)?;
    let dependency_rows = sqlx::query(
        "SELECT depends_on_number FROM issue_dependencies
         WHERE project_id = $1 AND issue_number = $2 ORDER BY depends_on_number",
    )
    .bind(project_id)
    .bind(issue_number)
    .fetch_all(&mut **tx)
    .await?;
    issue.dependencies = dependency_rows
        .into_iter()
        .map(|row| row.try_get::<i64, _>("depends_on_number"))
        .collect::<Result<Vec<_>, _>>()?;
    let fact_rows = sqlx::query(
        "SELECT fact_id, kind, value, description, satisfied
         FROM issue_blocking_facts
         WHERE project_id = $1 AND issue_number = $2 ORDER BY fact_id",
    )
    .bind(project_id)
    .bind(issue_number)
    .fetch_all(&mut **tx)
    .await?;
    issue.blocking_facts = fact_rows
        .iter()
        .map(blocking_fact_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(issue))
}

fn native_issue_from_row(row: &SqliteRow) -> Result<NativeIssue, DaemonError> {
    let criteria_json: String = row.try_get("acceptance_criteria_json")?;
    let external_references_json: String = row.try_get("external_references_json")?;
    let status: String = row.try_get("status")?;
    let verification_level: String = row.try_get("verification_level")?;
    let verification_steps_json: String = row.try_get("verification_steps_json")?;
    Ok(NativeIssue {
        issue_id: row.try_get("issue_id")?,
        project_id: row.try_get("project_id")?,
        issue_number: row.try_get("issue_number")?,
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        acceptance_criteria: serde_json::from_str(&criteria_json)
            .map_err(|error| corrupt_run(format!("invalid Issue acceptance criteria: {error}")))?,
        external_references: serde_json::from_str(&external_references_json)
            .map_err(|error| corrupt_run(format!("invalid Issue external references: {error}")))?,
        dependencies: Vec::new(),
        blocking_facts: Vec::new(),
        board_state: IssueBoardState::from_db(&status)?,
        revision: row.try_get("revision")?,
        changed_by_run_id: row.try_get("changed_by_run_id")?,
        closure_summary: row.try_get("closure_summary")?,
        created_at: row.try_get("created_at")?,
        started_at: row.try_get("started_at")?,
        updated_at: row.try_get("updated_at")?,
        closed_at: row.try_get("closed_at")?,
        archived_at: row.try_get("archived_at")?,
        verification_level: VerificationLevel::from_db(&verification_level)?,
        verification_steps: parse_verification_steps_json(&verification_steps_json)?,
        state_events: Vec::new(),
    })
}

fn parse_verification_steps_json(value: &str) -> Result<Vec<VerificationStep>, DaemonError> {
    let raw: serde_json::Value = serde_json::from_str(value)
        .map_err(|error| corrupt_run(format!("invalid Issue verification steps: {error}")))?;
    let items = match raw {
        serde_json::Value::Array(items) => items,
        _ => {
            return Err(corrupt_run(
                "invalid Issue verification steps: expected a JSON array".to_owned(),
            ));
        }
    };
    let mut steps = Vec::with_capacity(items.len());
    for item in items {
        match item {
            serde_json::Value::String(text) => {
                steps.push(VerificationStep {
                    text,
                    completed: false,
                });
            }
            serde_json::Value::Object(map) => {
                let text = map
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        corrupt_run(
                            "invalid Issue verification step: missing string text".to_owned(),
                        )
                    })?
                    .to_owned();
                let completed = match map.get("completed") {
                    Some(serde_json::Value::Bool(completed)) => *completed,
                    Some(serde_json::Value::Number(number)) => number.as_i64() == Some(1),
                    _ => false,
                };
                steps.push(VerificationStep { text, completed });
            }
            _ => {
                return Err(corrupt_run(
                    "invalid Issue verification step: expected a string or object".to_owned(),
                ));
            }
        }
    }
    Ok(steps)
}

async fn record_issue_state_event_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    issue_number: i64,
    from_state: IssueBoardState,
    to_state: IssueBoardState,
    changed_by_run_id: Option<&str>,
    now: &str,
) -> Result<(), DaemonError> {
    sqlx::query(
        "INSERT INTO issue_state_events (
            event_id, project_id, issue_number, from_state, to_state,
            changed_by_run_id, occurred_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(format!("isevt_{}", Uuid::new_v4().simple()))
    .bind(project_id)
    .bind(issue_number)
    .bind(from_state.as_str())
    .bind(to_state.as_str())
    .bind(changed_by_run_id)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn set_native_issue_state_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    issue_number: i64,
    board_state: IssueBoardState,
    changed_by_run_id: Option<&str>,
    closure_summary: Option<&str>,
    now: &str,
) -> Result<NativeIssue, DaemonError> {
    let mut issue = load_native_issue_tx(tx, project_id, issue_number)
        .await?
        .ok_or_else(|| DaemonError::NotFound(format!("ISSUE-{issue_number:03}")))?;
    if issue.board_state == board_state
        && issue.changed_by_run_id.as_deref() == changed_by_run_id
        && issue.closure_summary.as_deref() == closure_summary
        && (board_state != IssueBoardState::InProgress || issue.started_at.is_some())
    {
        return Ok(issue);
    }
    let from_state = issue.board_state;
    issue.board_state = board_state;
    issue.revision += 1;
    issue.changed_by_run_id = changed_by_run_id.map(str::to_owned);
    issue.closure_summary = closure_summary.map(str::to_owned);
    issue.updated_at = now.to_owned();
    if board_state == IssueBoardState::InProgress && issue.started_at.is_none() {
        issue.started_at = Some(now.to_owned());
    }
    issue.closed_at = (board_state == IssueBoardState::Done).then(|| now.to_owned());
    sqlx::query(
        "UPDATE native_issues
         SET status = $3, revision = $4, changed_by_run_id = $5,
             closure_summary = $6, updated_at = $7, started_at = $8,
             closed_at = $9
         WHERE project_id = $1 AND issue_number = $2",
    )
    .bind(project_id)
    .bind(issue_number)
    .bind(board_state.as_str())
    .bind(issue.revision)
    .bind(changed_by_run_id)
    .bind(closure_summary)
    .bind(now)
    .bind(issue.started_at.as_deref())
    .bind(issue.closed_at.as_deref())
    .execute(&mut **tx)
    .await?;
    record_issue_state_event_tx(
        tx,
        project_id,
        issue_number,
        from_state,
        board_state,
        changed_by_run_id,
        now,
    )
    .await?;
    Ok(issue)
}

fn validate_issue_create(request: &CreateIssueRequest) -> Result<(), DaemonError> {
    validate_required("project_id", &request.project_id, MAX_IDENTIFIER_BYTES)?;
    validate_required("title", &request.title, MAX_ISSUE_TITLE_BYTES)?;
    validate_required(
        "description",
        &request.description,
        MAX_ISSUE_DESCRIPTION_BYTES,
    )?;
    validate_issue_criteria(&request.acceptance_criteria)?;
    validate_dependency_keys(&request.dependencies)?;
    validate_blocking_facts(&request.blocking_facts)
}

fn validate_issue_update(request: &UpdateIssueRequest) -> Result<(), DaemonError> {
    validate_required("project_id", &request.project_id, MAX_IDENTIFIER_BYTES)?;
    parse_issue_reference(&request.issue_key)?;
    validate_revision(request.expected_revision)?;
    if request.title.is_none()
        && request.description.is_none()
        && request.acceptance_criteria.is_none()
        && request.external_references.is_none()
        && request.dependencies.is_none()
        && request.blocking_facts.is_none()
        && request.verification_level.is_none()
        && request.verification_steps.is_none()
    {
        return Err(DaemonError::InvalidRequest(
            "update must provide at least one semantic field".to_owned(),
        ));
    }
    if let Some(dependencies) = &request.dependencies {
        validate_dependency_keys(dependencies)?;
    }
    if let Some(blocking_facts) = &request.blocking_facts {
        validate_blocking_facts(blocking_facts)?;
    }
    Ok(())
}

fn validate_issue_criteria(criteria: &[String]) -> Result<(), DaemonError> {
    if criteria.len() > MAX_ISSUE_CRITERIA {
        return Err(DaemonError::InvalidRequest(format!(
            "acceptance_criteria must contain at most {MAX_ISSUE_CRITERIA} items"
        )));
    }
    for criterion in criteria {
        validate_required("acceptance criterion", criterion, MAX_ISSUE_CRITERION_BYTES)?;
    }
    Ok(())
}

fn normalize_issue_external_references(
    references: &[IssueExternalReference],
) -> Result<Vec<IssueExternalReference>, DaemonError> {
    if references.len() > MAX_ISSUE_EXTERNAL_REFERENCES {
        return Err(DaemonError::InvalidRequest(format!(
            "external_references must contain at most {MAX_ISSUE_EXTERNAL_REFERENCES} items"
        )));
    }

    let mut normalized = Vec::with_capacity(references.len());
    let mut seen = BTreeSet::new();
    for reference in references {
        let candidate = reference.url.trim();
        if candidate.is_empty() {
            return Err(DaemonError::InvalidRequest(
                "external reference url must not be empty".to_owned(),
            ));
        }
        if candidate.len() > MAX_ISSUE_EXTERNAL_REFERENCE_URL_BYTES {
            return Err(DaemonError::InvalidRequest(format!(
                "external reference url must not exceed {MAX_ISSUE_EXTERNAL_REFERENCE_URL_BYTES} UTF-8 bytes"
            )));
        }
        let parsed = reqwest::Url::parse(candidate).map_err(|error| {
            DaemonError::InvalidRequest(format!("invalid external reference url: {error}"))
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(DaemonError::InvalidRequest(
                "external reference url must use http or https".to_owned(),
            ));
        }
        if parsed.host_str().is_none() {
            return Err(DaemonError::InvalidRequest(
                "external reference url must include a host".to_owned(),
            ));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(DaemonError::InvalidRequest(
                "external reference url must not include credentials".to_owned(),
            ));
        }

        let url = parsed.to_string();
        if url.len() > MAX_ISSUE_EXTERNAL_REFERENCE_URL_BYTES {
            return Err(DaemonError::InvalidRequest(format!(
                "normalized external reference url must not exceed {MAX_ISSUE_EXTERNAL_REFERENCE_URL_BYTES} UTF-8 bytes"
            )));
        }
        if seen.insert((reference.kind, url.clone())) {
            normalized.push(IssueExternalReference {
                kind: reference.kind,
                url,
            });
        }
    }
    Ok(normalized)
}

fn validate_dependency_keys(keys: &[String]) -> Result<(), DaemonError> {
    if keys.len() > MAX_ISSUE_DEPENDENCIES {
        return Err(DaemonError::InvalidRequest(format!(
            "dependencies must contain at most {MAX_ISSUE_DEPENDENCIES} items"
        )));
    }
    let mut seen = BTreeSet::new();
    for key in keys {
        let number = parse_issue_reference(key)?;
        if !seen.insert(number) {
            return Err(DaemonError::InvalidRequest(format!(
                "duplicate dependency {key}"
            )));
        }
    }
    Ok(())
}

fn parse_dependencies(project_id: &str, keys: &[String]) -> Result<Vec<i64>, DaemonError> {
    validate_required("project_id", project_id, MAX_IDENTIFIER_BYTES)?;
    validate_dependency_keys(keys)?;
    keys.iter().map(|key| parse_issue_reference(key)).collect()
}

fn validate_blocking_facts(facts: &[IssueBlockingFact]) -> Result<(), DaemonError> {
    if facts.len() > MAX_ISSUE_BLOCKING_FACTS {
        return Err(DaemonError::InvalidRequest(format!(
            "blocking_facts must contain at most {MAX_ISSUE_BLOCKING_FACTS} items"
        )));
    }
    let mut seen = BTreeSet::new();
    for fact in facts {
        if fact.fact_id.trim().is_empty() {
            return Err(DaemonError::InvalidRequest(
                "blocking fact fact_id must not be empty".to_owned(),
            ));
        }
        if fact.fact_id.len() > MAX_ISSUE_FACT_ID_BYTES {
            return Err(DaemonError::InvalidRequest(format!(
                "blocking fact fact_id must not exceed {MAX_ISSUE_FACT_ID_BYTES} UTF-8 bytes"
            )));
        }
        if fact.description.trim().is_empty() {
            return Err(DaemonError::InvalidRequest(
                "blocking fact description must not be empty".to_owned(),
            ));
        }
        if fact.description.len() > MAX_ISSUE_FACT_DESCRIPTION_BYTES {
            return Err(DaemonError::InvalidRequest(format!(
                "blocking fact description must not exceed {MAX_ISSUE_FACT_DESCRIPTION_BYTES} UTF-8 bytes"
            )));
        }
        if let Some(value) = &fact.value
            && value.len() > MAX_ISSUE_FACT_VALUE_BYTES
        {
            return Err(DaemonError::InvalidRequest(format!(
                "blocking fact value must not exceed {MAX_ISSUE_FACT_VALUE_BYTES} UTF-8 bytes"
            )));
        }
        if !seen.insert(fact.fact_id.trim()) {
            return Err(DaemonError::InvalidRequest(format!(
                "duplicate blocking fact {}",
                fact.fact_id
            )));
        }
    }
    Ok(())
}

fn normalize_issue_blocking_facts(
    facts: &[IssueBlockingFact],
) -> Result<Vec<IssueBlockingFact>, DaemonError> {
    validate_blocking_facts(facts)?;
    Ok(facts
        .iter()
        .map(|fact| IssueBlockingFact {
            fact_id: fact.fact_id.trim().to_owned(),
            kind: fact.kind,
            value: fact.value.as_deref().map(str::trim).map(str::to_owned),
            description: fact.description.trim().to_owned(),
            satisfied: fact.satisfied,
        })
        .collect())
}

async fn validate_dependencies_exist_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    dependencies: &[i64],
    subject_issue_number: Option<i64>,
) -> Result<(), DaemonError> {
    for &depends_on_number in dependencies {
        if subject_issue_number == Some(depends_on_number) {
            return Err(DaemonError::InvalidRequest(format!(
                "ISSUE-{depends_on_number:03} cannot depend on itself"
            )));
        }
        let exists: Option<i64> = sqlx::query_scalar(
            "SELECT issue_number FROM native_issues
             WHERE project_id = $1 AND issue_number = $2 AND archived_at IS NULL",
        )
        .bind(project_id)
        .bind(depends_on_number)
        .fetch_optional(&mut **tx)
        .await?;
        if exists.is_none() {
            return Err(DaemonError::NotFound(format!(
                "dependency ISSUE-{depends_on_number:03}"
            )));
        }
    }
    Ok(())
}

/// Checks that adding `new_dependencies` for `subject_issue_number` keeps the
/// whole Project dependency graph acyclic using Kahn's topological sort: a
/// cycle exists iff some node never reaches indegree zero.
async fn assert_dependency_graph_acyclic_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    subject_issue_number: i64,
    new_dependencies: &[i64],
) -> Result<(), DaemonError> {
    let rows = sqlx::query(
        "SELECT issue_number, depends_on_number
         FROM issue_dependencies WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut adjacency = BTreeMap::<i64, BTreeSet<i64>>::new();
    for row in rows {
        let issue_number: i64 = row.try_get("issue_number")?;
        let depends_on_number: i64 = row.try_get("depends_on_number")?;
        adjacency
            .entry(issue_number)
            .or_default()
            .insert(depends_on_number);
    }
    adjacency
        .entry(subject_issue_number)
        .or_default()
        .extend(new_dependencies.iter().copied());

    let mut indegrees = BTreeMap::<i64, usize>::new();
    for (&node, successors) in &adjacency {
        indegrees.entry(node).or_default();
        for successor in successors {
            *indegrees.entry(*successor).or_default() += 1;
        }
    }
    let mut queue = indegrees
        .iter()
        .filter(|(_, indegree)| **indegree == 0)
        .map(|(node, _)| *node)
        .collect::<std::collections::VecDeque<_>>();
    let mut visited = 0usize;
    while let Some(node) = queue.pop_front() {
        visited += 1;
        if let Some(successors) = adjacency.get(&node) {
            for successor in successors {
                let indegree = indegrees
                    .get_mut(successor)
                    .expect("successor has indegree");
                *indegree -= 1;
                if *indegree == 0 {
                    queue.push_back(*successor);
                }
            }
        }
    }
    if visited != indegrees.len() {
        let stuck = indegrees
            .iter()
            .find(|(_, indegree)| **indegree > 0)
            .map(|(node, _)| *node)
            .unwrap_or(subject_issue_number);
        return Err(DaemonError::InvalidRequest(format!(
            "dependencies would create a cycle involving ISSUE-{stuck:03}"
        )));
    }
    Ok(())
}

async fn insert_dependencies_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    issue_number: i64,
    dependencies: &[i64],
    now: &str,
) -> Result<(), DaemonError> {
    for &depends_on_number in dependencies {
        sqlx::query(
            "INSERT INTO issue_dependencies (project_id, issue_number, depends_on_number, created_at)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(project_id)
        .bind(issue_number)
        .bind(depends_on_number)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn insert_blocking_facts_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    issue_number: i64,
    facts: &[IssueBlockingFact],
    now: &str,
) -> Result<(), DaemonError> {
    for fact in facts {
        sqlx::query(
            "INSERT INTO issue_blocking_facts (
                project_id, issue_number, fact_id, kind, value,
                description, satisfied, created_at, updated_at
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)",
        )
        .bind(project_id)
        .bind(issue_number)
        .bind(&fact.fact_id)
        .bind(fact.kind.as_str())
        .bind(&fact.value)
        .bind(&fact.description)
        .bind(fact.satisfied)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn parse_legacy_issue_content(content: &str) -> (String, Vec<String>) {
    let lines = content.lines().collect::<Vec<_>>();
    let mut start = 0;
    if lines
        .first()
        .is_some_and(|line| line.trim_start().starts_with("# "))
    {
        start = 1;
    }
    while lines.get(start).is_some_and(|line| line.trim().is_empty()) {
        start += 1;
    }
    if lines
        .get(start)
        .is_some_and(|line| line.trim().starts_with('|'))
    {
        while lines
            .get(start)
            .is_some_and(|line| line.trim().starts_with('|'))
        {
            start += 1;
        }
        while lines.get(start).is_some_and(|line| line.trim().is_empty()) {
            start += 1;
        }
    }

    let body = &lines[start..];
    let criteria_start = body.iter().position(|line| {
        let heading = line.trim().trim_start_matches('#').trim();
        heading.eq_ignore_ascii_case("acceptance criteria")
            || matches!(heading, "验收标准" | "验收条件")
    });
    let Some(criteria_start) = criteria_start else {
        return (body.join("\n").trim().to_owned(), Vec::new());
    };
    let criteria_end = body[criteria_start + 1..]
        .iter()
        .position(|line| line.trim_start().starts_with('#'))
        .map_or(body.len(), |offset| criteria_start + 1 + offset);
    let criteria = body[criteria_start + 1..criteria_end]
        .iter()
        .filter_map(|line| {
            let line = line.trim();
            ["- [ ] ", "- [x] ", "- [X] ", "- "]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
                .map(str::trim)
                .filter(|criterion| !criterion.is_empty())
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    if criteria.is_empty() {
        return (body.join("\n").trim().to_owned(), criteria);
    }
    let description = body[..criteria_start]
        .iter()
        .chain(&body[criteria_end..])
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    (description, criteria)
}

fn ensure_issue_revision(issue: &NativeIssue, expected: i64) -> Result<(), DaemonError> {
    if issue.revision != expected {
        return Err(run_conflict(format!(
            "ISSUE-{:03} revision is {}, expected {expected}",
            issue.issue_number, issue.revision
        )));
    }
    Ok(())
}

async fn bind_unlinked_children(
    tx: &mut Transaction<'_, Sqlite>,
    parent: &AgentRun,
    issue_number: i64,
    now: &str,
) -> Result<(), DaemonError> {
    let child_rows = sqlx::query(
        "SELECT run_id, project_id, issue_number, host, host_run_key, host_session_id,
                parent_run_id, kind, phase, outcome, end_reason, display_label, summary,
                revision, started_at, last_seen_at, lease_expires_at, ended_at
         FROM agent_runs
         WHERE parent_run_id = $1 AND issue_number IS NULL
         ORDER BY run_id",
    )
    .bind(&parent.run_id)
    .fetch_all(&mut **tx)
    .await?;
    for row in child_rows {
        let mut child = agent_run_from_row(&row)?;
        child.issue_number = Some(issue_number);
        child.revision += 1;
        child.last_seen_at = now.to_owned();
        update_run(tx, &child).await?;
        insert_event(
            tx,
            &format!("arevt_{}", Uuid::new_v4().simple()),
            None,
            Some(&child.run_id),
            child.host_session_id.as_deref(),
            AgentRunEventType::IssueBound,
            AgentRunEventSource::Mcp,
            Some(issue_number),
            None,
            None,
            now,
        )
        .await?;
    }
    Ok(())
}

fn issue_workflow_mutation_response(
    run: Option<AgentRun>,
    issue: &NativeIssue,
) -> IssueWorkflowMutationResponse {
    IssueWorkflowMutationResponse {
        issue_id: issue.issue_id.clone(),
        issue_key: format!("ISSUE-{:03}", issue.issue_number),
        board_state: issue.board_state,
        state_revision: issue.revision,
        state_updated_at: issue.updated_at.clone(),
        run,
    }
}

async fn load_issue_workflow_state_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    issue_number: i64,
) -> Result<Option<IssueWorkflowState>, DaemonError> {
    let row = sqlx::query(
        "SELECT issue_number, open_state, observed_lifecycle, revision,
                changed_by_run_id, summary, updated_at
         FROM issue_workflow_states
         WHERE project_id = $1 AND issue_number = $2",
    )
    .bind(project_id)
    .bind(issue_number)
    .fetch_optional(&mut **tx)
    .await?;
    row.as_ref().map(issue_workflow_state_from_row).transpose()
}

fn issue_workflow_state_from_row(row: &SqliteRow) -> Result<IssueWorkflowState, DaemonError> {
    let open_state: String = row.try_get("open_state")?;
    let observed_lifecycle: String = row.try_get("observed_lifecycle")?;
    Ok(IssueWorkflowState {
        issue_number: row.try_get("issue_number")?,
        board_state: IssueBoardState::from_open_db(&open_state)?,
        observed_lifecycle: IssueLifecycle::from_db(&observed_lifecycle)?,
        revision: row.try_get("revision")?,
        updated_at: row.try_get("updated_at")?,
        changed_by_run_id: row.try_get("changed_by_run_id")?,
        summary: row.try_get("summary")?,
    })
}

#[cfg(test)]
async fn set_issue_workflow_state_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    issue_number: i64,
    board_state: IssueBoardState,
    changed_by_run_id: Option<&str>,
    summary: Option<&str>,
    now: &str,
) -> Result<IssueWorkflowState, DaemonError> {
    let open_state = board_state.as_open_str()?;
    let current = load_issue_workflow_state_tx(tx, project_id, issue_number).await?;
    if let Some(current) = &current
        && current.board_state == board_state
        && current.observed_lifecycle == IssueLifecycle::Open
        && current.changed_by_run_id.as_deref() == changed_by_run_id
        && current.summary.as_deref() == summary
    {
        return Ok(current.clone());
    }
    let revision = current.map_or(1, |state| state.revision + 1);
    sqlx::query(
        "INSERT INTO issue_workflow_states (
            project_id, issue_number, open_state, observed_lifecycle, revision,
            changed_by_run_id, summary, updated_at
         ) VALUES ($1, $2, $3, 'open', $4, $5, $6, $7)
         ON CONFLICT(project_id, issue_number) DO UPDATE SET
            open_state = excluded.open_state,
            observed_lifecycle = excluded.observed_lifecycle,
            revision = excluded.revision,
            changed_by_run_id = excluded.changed_by_run_id,
            summary = excluded.summary,
            updated_at = excluded.updated_at",
    )
    .bind(project_id)
    .bind(issue_number)
    .bind(open_state)
    .bind(revision)
    .bind(changed_by_run_id)
    .bind(summary)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(IssueWorkflowState {
        issue_number,
        board_state,
        observed_lifecycle: IssueLifecycle::Open,
        revision,
        updated_at: now.to_owned(),
        changed_by_run_id: changed_by_run_id.map(str::to_owned),
        summary: summary.map(str::to_owned),
    })
}

pub(crate) async fn reconcile_issue_workflow_states(
    pool: &SqlitePool,
    project_id: &str,
    cards: &mut [IssueBoardCard],
    stale_before: &str,
) -> Result<(), DaemonError> {
    validate_required("project_id", project_id, MAX_IDENTIFIER_BYTES)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let (now, _) = db_clock(&mut tx).await?;
    for card in cards {
        let current = load_issue_workflow_state_tx(&mut tx, project_id, card.issue_number).await?;
        let state = match current {
            None => {
                sqlx::query(
                    "INSERT INTO issue_workflow_states (
                        project_id, issue_number, open_state, observed_lifecycle, revision,
                        changed_by_run_id, summary, updated_at
                     ) VALUES ($1, $2, 'todo', $3, 1, NULL, NULL, $4)",
                )
                .bind(project_id)
                .bind(card.issue_number)
                .bind(card.lifecycle.as_str())
                .bind(&now)
                .execute(&mut *tx)
                .await?;
                IssueWorkflowState {
                    issue_number: card.issue_number,
                    board_state: IssueBoardState::Todo,
                    observed_lifecycle: card.lifecycle,
                    revision: 1,
                    updated_at: now.clone(),
                    changed_by_run_id: None,
                    summary: None,
                }
            }
            Some(mut state) if state.observed_lifecycle != card.lifecycle => {
                let reopened = state.observed_lifecycle == IssueLifecycle::Closed
                    && card.lifecycle == IssueLifecycle::Open;
                if reopened {
                    state.board_state = IssueBoardState::Todo;
                    state.changed_by_run_id = None;
                    state.summary = None;
                }
                state.observed_lifecycle = card.lifecycle;
                state.revision += 1;
                state.updated_at = now.clone();
                sqlx::query(
                    "UPDATE issue_workflow_states
                     SET open_state = $3, observed_lifecycle = $4, revision = $5,
                         changed_by_run_id = $6, summary = $7, updated_at = $8
                     WHERE project_id = $1 AND issue_number = $2",
                )
                .bind(project_id)
                .bind(card.issue_number)
                .bind(state.board_state.as_open_str()?)
                .bind(state.observed_lifecycle.as_str())
                .bind(state.revision)
                .bind(&state.changed_by_run_id)
                .bind(&state.summary)
                .bind(&state.updated_at)
                .execute(&mut *tx)
                .await?;
                state
            }
            Some(state) => state,
        };
        card.board_state = if card.lifecycle == IssueLifecycle::Closed {
            IssueBoardState::Done
        } else {
            state.board_state
        };
        card.state_revision = state.revision;
        card.state_updated_at = Some(state.updated_at.clone());
        card.closure_summary = state.summary.clone();
        let latest_activity = card
            .latest_run
            .as_ref()
            .map(meaningful_timestamp)
            .unwrap_or(state.updated_at.as_str());
        card.is_stale = card.board_state == IssueBoardState::InProgress
            && card.active_runs.is_empty()
            && latest_activity <= stale_before;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
async fn load_agent_run(pool: &SqlitePool, run_id: &str) -> Result<Option<AgentRun>, DaemonError> {
    let row = sqlx::query(
        "SELECT run_id, project_id, issue_number, host, host_run_key, host_session_id,
                parent_run_id, kind, phase, outcome, end_reason, display_label, summary,
                revision, started_at, last_seen_at, lease_expires_at, ended_at
         FROM agent_runs WHERE run_id = $1",
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(agent_run_from_row).transpose()
}

pub(crate) async fn load_project_runs(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<Vec<AgentRun>, DaemonError> {
    validate_required("project_id", project_id, MAX_IDENTIFIER_BYTES)?;
    let rows = sqlx::query(
        "SELECT run_id, project_id, issue_number, host, host_run_key, host_session_id,
                parent_run_id, kind, phase, outcome, end_reason, display_label, summary,
                revision, started_at, last_seen_at, lease_expires_at, ended_at
         FROM agent_runs WHERE project_id = $1
         ORDER BY COALESCE(ended_at, last_seen_at) DESC, run_id DESC",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    rows.iter().map(agent_run_from_row).collect()
}

pub(crate) async fn load_agent_run_for_project(
    pool: &SqlitePool,
    project_id: &str,
    run_id: &str,
) -> Result<Option<AgentRun>, DaemonError> {
    validate_required("project_id", project_id, MAX_IDENTIFIER_BYTES)?;
    validate_required("run_id", run_id, MAX_IDENTIFIER_BYTES)?;
    let row = sqlx::query(
        "SELECT run_id, project_id, issue_number, host, host_run_key, host_session_id,
                parent_run_id, kind, phase, outcome, end_reason, display_label, summary,
                revision, started_at, last_seen_at, lease_expires_at, ended_at
         FROM agent_runs WHERE project_id = $1 AND run_id = $2",
    )
    .bind(project_id)
    .bind(run_id)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(agent_run_from_row).transpose()
}

pub(crate) const ISSUE_CLOSED_REASON: &str = "issue_closed";

/// Persistently ends runs that can no longer make progress: running runs
/// whose lease expired (no heartbeat for 24h) and running runs bound to a
/// Done Issue. The lease-expired transition was previously only derived at
/// read time, so Activity showed Running for up to a day and then Unknown
/// forever; ending it in the DB keeps every read consistent.
pub(crate) async fn recover_stale_runs(pool: &SqlitePool) -> Result<u64, DaemonError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let rows = sqlx::query(
        "SELECT run_id, project_id, issue_number, host, host_run_key, host_session_id,
                parent_run_id, kind, phase, outcome, end_reason, display_label, summary,
                revision, started_at, last_seen_at, lease_expires_at, ended_at
         FROM agent_runs
         WHERE phase = 'running'
           AND (lease_expires_at <= strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                OR issue_number IN (
                    SELECT issue_number FROM native_issues
                    WHERE project_id = agent_runs.project_id AND status = 'done'
                ))
         ORDER BY lease_expires_at, run_id",
    )
    .fetch_all(&mut *tx)
    .await?;
    let count = rows.len() as u64;
    let (now, _) = db_clock(&mut tx).await?;
    for row in rows {
        let mut run = agent_run_from_row(&row)?;
        let timed_out = run.lease_expires_at.as_str() <= now.as_str();
        run.phase = AgentRunPhase::Ended;
        run.outcome = Some(AgentRunOutcome::Unknown);
        run.end_reason = Some(
            if timed_out {
                LEASE_EXPIRED_REASON
            } else {
                ISSUE_CLOSED_REASON
            }
            .to_owned(),
        );
        run.revision += 1;
        run.ended_at = Some(if timed_out {
            run.lease_expires_at.clone()
        } else {
            now.clone()
        });
        update_run(&mut tx, &run).await?;
        insert_event(
            &mut tx,
            &format!("arevt_{}", Uuid::new_v4().simple()),
            None,
            Some(&run.run_id),
            run.host_session_id.as_deref(),
            AgentRunEventType::Ended,
            AgentRunEventSource::Recovery,
            run.issue_number,
            Some(AgentRunOutcome::Unknown),
            None,
            &run.lease_expires_at,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(count)
}

fn validate_record_request(request: &RecordAgentRunEventRequest) -> Result<(), DaemonError> {
    validate_required("event_id", &request.event_id, MAX_IDENTIFIER_BYTES)?;
    validate_required("project_id", &request.project_id, MAX_IDENTIFIER_BYTES)?;
    validate_optional(
        "host_run_key",
        request.host_run_key.as_deref(),
        MAX_RUN_KEY_BYTES,
    )?;
    validate_optional(
        "host_session_id",
        request.host_session_id.as_deref(),
        MAX_IDENTIFIER_BYTES,
    )?;
    validate_optional(
        "parent_run_id",
        request.parent_run_id.as_deref(),
        MAX_IDENTIFIER_BYTES,
    )?;
    validate_optional(
        "parent_host_run_key",
        request.parent_host_run_key.as_deref(),
        MAX_RUN_KEY_BYTES,
    )?;
    validate_optional(
        "display_label",
        request.display_label.as_deref(),
        MAX_DISPLAY_LABEL_BYTES,
    )?;
    validate_optional("summary", request.summary.as_deref(), MAX_SUMMARY_BYTES)?;
    validate_optional("occurred_at", request.occurred_at.as_deref(), 64)?;
    if let Some(issue_key) = request.issue_key.as_deref() {
        parse_issue_reference(issue_key)?;
    }
    match request.event_type {
        AgentRunEventType::SessionEnded => {
            if request.host_run_key.is_some() {
                return Err(DaemonError::InvalidRequest(
                    "session_ended must not include host_run_key".to_owned(),
                ));
            }
            if request.host_session_id.is_none() {
                return Err(DaemonError::InvalidRequest(
                    "session_ended requires host_session_id".to_owned(),
                ));
            }
            if !matches!(request.outcome, None | Some(AgentRunOutcome::Unknown)) {
                return Err(DaemonError::InvalidRequest(
                    "session_ended outcome must be unknown".to_owned(),
                ));
            }
        }
        AgentRunEventType::Started | AgentRunEventType::Heartbeat => {
            if request.host_run_key.is_none() {
                return Err(DaemonError::InvalidRequest(
                    "run lifecycle events require host_run_key".to_owned(),
                ));
            }
            if request.outcome.is_some() {
                return Err(DaemonError::InvalidRequest(
                    "start and heartbeat events must not include outcome".to_owned(),
                ));
            }
        }
        AgentRunEventType::Ended => {
            if request.host_run_key.is_none() {
                return Err(DaemonError::InvalidRequest(
                    "run lifecycle events require host_run_key".to_owned(),
                ));
            }
        }
        AgentRunEventType::IssueBound | AgentRunEventType::OutcomeReported => {
            return Err(DaemonError::InvalidRequest(
                "semantic Issue events are written through start/closure operations".to_owned(),
            ));
        }
    }
    if request.kind == Some(AgentRunKind::Root)
        && (request.parent_run_id.is_some() || request.parent_host_run_key.is_some())
    {
        return Err(DaemonError::InvalidRequest(
            "a root AgentRun cannot have a parent".to_owned(),
        ));
    }
    Ok(())
}

fn validate_required(name: &str, value: &str, max_bytes: usize) -> Result<(), DaemonError> {
    if value.trim().is_empty() {
        return Err(DaemonError::InvalidRequest(format!(
            "{name} must not be empty"
        )));
    }
    if value.len() > max_bytes {
        return Err(DaemonError::InvalidRequest(format!(
            "{name} must not exceed {max_bytes} UTF-8 bytes"
        )));
    }
    Ok(())
}

fn validate_optional(name: &str, value: Option<&str>, max_bytes: usize) -> Result<(), DaemonError> {
    if let Some(value) = value {
        validate_required(name, value, max_bytes)?;
    }
    Ok(())
}

fn validate_revision(revision: i64) -> Result<(), DaemonError> {
    if revision < 1 {
        return Err(DaemonError::InvalidRequest(
            "expected_revision must be a positive integer".to_owned(),
        ));
    }
    Ok(())
}

fn validate_issue_id(issue_id: &str) -> Result<(), DaemonError> {
    validate_required("issue_id", issue_id, MAX_IDENTIFIER_BYTES)?;
    let suffix = issue_id.strip_prefix("issue_").ok_or_else(|| {
        DaemonError::InvalidRequest(
            "issue_id must match issue_<32 lowercase hexadecimal characters>".to_owned(),
        )
    })?;
    if suffix.len() != 32
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(DaemonError::InvalidRequest(
            "issue_id must match issue_<32 lowercase hexadecimal characters>".to_owned(),
        ));
    }
    Ok(())
}

fn ensure_revision(run: &AgentRun, expected_revision: i64) -> Result<(), DaemonError> {
    if run.revision != expected_revision {
        return Err(run_conflict(format!(
            "AgentRun {} changed from revision {expected_revision} to {}",
            run.run_id, run.revision
        )));
    }
    Ok(())
}

pub(crate) fn parse_issue_reference(issue_key: &str) -> Result<i64, DaemonError> {
    let digits = issue_key.strip_prefix("ISSUE-").ok_or_else(|| {
        DaemonError::InvalidRequest("issue_key must use the ISSUE-NNN form".to_owned())
    })?;
    if digits.len() != 3
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || digits.bytes().all(|byte| byte == b'0')
    {
        return Err(DaemonError::InvalidRequest(
            "issue_key must use exactly three digits and must not be ISSUE-000".to_owned(),
        ));
    }
    digits.parse::<i64>().map_err(|_| {
        DaemonError::InvalidRequest("issue_key contains an invalid Issue number".to_owned())
    })
}

fn canonical_json_fingerprint(value: &impl Serialize) -> Result<String, DaemonError> {
    let value = serde_json::to_value(value)?;
    let mut canonical = String::new();
    write_canonical_json(&value, &mut canonical)?;
    Ok(format!(
        "sha256:{}",
        hex::encode(Sha256::digest(canonical.as_bytes()))
    ))
}

fn write_canonical_json(value: &serde_json::Value, output: &mut String) -> Result<(), DaemonError> {
    match value {
        serde_json::Value::Null => output.push_str("null"),
        serde_json::Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        serde_json::Value::Number(value) => output.push_str(&value.to_string()),
        serde_json::Value::String(value) => output.push_str(&serde_json::to_string(value)?),
        serde_json::Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(']');
        }
        serde_json::Value::Object(values) => {
            output.push('{');
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(&serde_json::to_string(key)?);
                output.push(':');
                write_canonical_json(&values[key], output)?;
            }
            output.push('}');
        }
    }
    Ok(())
}

async fn db_clock(tx: &mut Transaction<'_, Sqlite>) -> Result<(String, String), DaemonError> {
    let row = sqlx::query(
        "SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now') AS now,
                strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+24 hours') AS lease_expires_at",
    )
    .fetch_one(&mut **tx)
    .await?;
    Ok((row.try_get("now")?, row.try_get("lease_expires_at")?))
}

async fn normalize_occurred_at(
    tx: &mut Transaction<'_, Sqlite>,
    value: Option<&str>,
    fallback: &str,
) -> Result<String, DaemonError> {
    let Some(value) = value else {
        return Ok(fallback.to_owned());
    };
    let normalized: Option<String> =
        sqlx::query_scalar("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', $1)")
            .bind(value)
            .fetch_one(&mut **tx)
            .await?;
    normalized.ok_or_else(|| {
        DaemonError::InvalidRequest("occurred_at must be a valid RFC 3339 timestamp".to_owned())
    })
}

#[derive(Debug)]
struct ExistingAgentRunEvent {
    run_id: Option<String>,
    event_fingerprint: String,
}

async fn load_existing_event(
    tx: &mut Transaction<'_, Sqlite>,
    event_id: &str,
) -> Result<Option<ExistingAgentRunEvent>, DaemonError> {
    let row = sqlx::query(
        "SELECT run_id, event_fingerprint
         FROM agent_run_events WHERE event_id = $1",
    )
    .bind(event_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| {
        Ok(ExistingAgentRunEvent {
            run_id: row.try_get("run_id")?,
            event_fingerprint: row.try_get("event_fingerprint")?,
        })
    })
    .transpose()
}

async fn duplicate_run_event_response(
    tx: &mut Transaction<'_, Sqlite>,
    request: &RecordAgentRunEventRequest,
    run_id: Option<&str>,
) -> Result<RecordAgentRunEventResponse, DaemonError> {
    let run_id = run_id.ok_or_else(|| {
        corrupt_run(format!(
            "non-session event {} does not reference an AgentRun",
            request.event_id
        ))
    })?;
    let run = load_agent_run_tx(tx, run_id).await?.ok_or_else(|| {
        corrupt_run(format!(
            "event {} references missing AgentRun {run_id}",
            request.event_id
        ))
    })?;
    if run.project_id != request.project_id || run.host != request.host {
        return Err(corrupt_run(format!(
            "event {} fingerprint points to an inconsistent AgentRun",
            request.event_id
        )));
    }
    Ok(RecordAgentRunEventResponse {
        run: Some(run.clone()),
        affected_runs: vec![run],
        duplicate: true,
    })
}

async fn resolve_parent_run(
    tx: &mut Transaction<'_, Sqlite>,
    request: &RecordAgentRunEventRequest,
    kind: AgentRunKind,
) -> Result<Option<AgentRun>, DaemonError> {
    if kind == AgentRunKind::Root {
        return Ok(None);
    }
    let by_id = if let Some(parent_run_id) = request.parent_run_id.as_deref() {
        Some(
            load_agent_run_tx(tx, parent_run_id)
                .await?
                .ok_or_else(|| DaemonError::NotFound(format!("AgentRun {parent_run_id}")))?,
        )
    } else {
        None
    };
    let by_host_key = if let Some(parent_host_run_key) = request.parent_host_run_key.as_deref() {
        load_run_by_host_key(tx, &request.project_id, request.host, parent_host_run_key).await?
    } else {
        None
    };
    if let (Some(by_id), Some(by_host_key)) = (&by_id, &by_host_key)
        && by_id.run_id != by_host_key.run_id
    {
        return Err(run_conflict(
            "parent_run_id and parent_host_run_key resolve to different runs".to_owned(),
        ));
    }
    let parent = by_id.or(by_host_key);
    if let Some(parent) = &parent {
        if parent.project_id != request.project_id || parent.host != request.host {
            return Err(run_conflict(
                "an AgentRun parent must belong to the same project and host".to_owned(),
            ));
        }
        if parent.kind != AgentRunKind::Root {
            return Err(run_conflict(
                "a subagent AgentRun parent must be a root run".to_owned(),
            ));
        }
    }
    Ok(parent)
}

async fn load_run_by_host_key(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    host: AgentRunHost,
    host_run_key: &str,
) -> Result<Option<AgentRun>, DaemonError> {
    let row = sqlx::query(
        "SELECT run_id, project_id, issue_number, host, host_run_key, host_session_id,
                parent_run_id, kind, phase, outcome, end_reason, display_label, summary,
                revision, started_at, last_seen_at, lease_expires_at, ended_at
         FROM agent_runs
         WHERE project_id = $1 AND host = $2 AND host_run_key = $3",
    )
    .bind(project_id)
    .bind(host.as_str())
    .bind(host_run_key)
    .fetch_optional(&mut **tx)
    .await?;
    row.as_ref().map(agent_run_from_row).transpose()
}

async fn load_agent_run_tx(
    tx: &mut Transaction<'_, Sqlite>,
    run_id: &str,
) -> Result<Option<AgentRun>, DaemonError> {
    let row = sqlx::query(
        "SELECT run_id, project_id, issue_number, host, host_run_key, host_session_id,
                parent_run_id, kind, phase, outcome, end_reason, display_label, summary,
                revision, started_at, last_seen_at, lease_expires_at, ended_at
         FROM agent_runs WHERE run_id = $1",
    )
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.as_ref().map(agent_run_from_row).transpose()
}

async fn load_agent_run_for_project_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    run_id: &str,
) -> Result<Option<AgentRun>, DaemonError> {
    let row = sqlx::query(
        "SELECT run_id, project_id, issue_number, host, host_run_key, host_session_id,
                parent_run_id, kind, phase, outcome, end_reason, display_label, summary,
                revision, started_at, last_seen_at, lease_expires_at, ended_at
         FROM agent_runs WHERE project_id = $1 AND run_id = $2",
    )
    .bind(project_id)
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.as_ref().map(agent_run_from_row).transpose()
}

async fn insert_run(
    tx: &mut Transaction<'_, Sqlite>,
    run: &AgentRun,
    start_observed: bool,
) -> Result<(), DaemonError> {
    sqlx::query(
        "INSERT INTO agent_runs (
            run_id, project_id, issue_number, host, host_run_key, host_session_id,
            parent_run_id, kind, phase, outcome, end_reason, display_label, summary,
            revision, start_observed, started_at, last_seen_at, lease_expires_at, ended_at
         ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13,
            $14, $15, $16, $17, $18, $19
         )",
    )
    .bind(&run.run_id)
    .bind(&run.project_id)
    .bind(run.issue_number)
    .bind(run.host.as_str())
    .bind(&run.host_run_key)
    .bind(&run.host_session_id)
    .bind(&run.parent_run_id)
    .bind(run.kind.as_str())
    .bind(run.phase.as_str())
    .bind(run.outcome.map(AgentRunOutcome::as_str))
    .bind(&run.end_reason)
    .bind(&run.display_label)
    .bind(&run.summary)
    .bind(run.revision)
    .bind(start_observed)
    .bind(&run.started_at)
    .bind(&run.last_seen_at)
    .bind(&run.lease_expires_at)
    .bind(&run.ended_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn update_run(tx: &mut Transaction<'_, Sqlite>, run: &AgentRun) -> Result<(), DaemonError> {
    let result = sqlx::query(
        "UPDATE agent_runs
         SET issue_number = $2, host_session_id = $3, parent_run_id = $4,
             kind = $5, phase = $6, outcome = $7, end_reason = $8,
             display_label = $9, summary = $10, revision = $11,
             last_seen_at = $12, lease_expires_at = $13, ended_at = $14
         WHERE run_id = $1",
    )
    .bind(&run.run_id)
    .bind(run.issue_number)
    .bind(&run.host_session_id)
    .bind(&run.parent_run_id)
    .bind(run.kind.as_str())
    .bind(run.phase.as_str())
    .bind(run.outcome.map(AgentRunOutcome::as_str))
    .bind(&run.end_reason)
    .bind(&run.display_label)
    .bind(&run.summary)
    .bind(run.revision)
    .bind(&run.last_seen_at)
    .bind(&run.lease_expires_at)
    .bind(&run.ended_at)
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() != 1 {
        return Err(DaemonError::NotFound(format!("AgentRun {}", run.run_id)));
    }
    Ok(())
}

#[derive(Serialize)]
struct StoredAgentRunEventFingerprint<'a> {
    event_id: &'a str,
    run_id: Option<&'a str>,
    host_session_id: Option<&'a str>,
    event_type: AgentRunEventType,
    source: AgentRunEventSource,
    issue_number: Option<i64>,
    outcome: Option<AgentRunOutcome>,
    summary: Option<&'a str>,
    occurred_at: &'a str,
}

#[allow(clippy::too_many_arguments)]
async fn insert_event(
    tx: &mut Transaction<'_, Sqlite>,
    event_id: &str,
    supplied_fingerprint: Option<&str>,
    run_id: Option<&str>,
    host_session_id: Option<&str>,
    event_type: AgentRunEventType,
    source: AgentRunEventSource,
    issue_number: Option<i64>,
    outcome: Option<AgentRunOutcome>,
    summary: Option<&str>,
    occurred_at: &str,
) -> Result<(), DaemonError> {
    let generated_fingerprint = if supplied_fingerprint.is_none() {
        Some(canonical_json_fingerprint(
            &StoredAgentRunEventFingerprint {
                event_id,
                run_id,
                host_session_id,
                event_type,
                source,
                issue_number,
                outcome,
                summary,
                occurred_at,
            },
        )?)
    } else {
        None
    };
    let event_fingerprint = supplied_fingerprint
        .or(generated_fingerprint.as_deref())
        .expect("an event fingerprint is always supplied or generated");
    sqlx::query(
        "INSERT INTO agent_run_events (
            event_id, event_fingerprint, run_id, host_session_id, event_type, source,
            issue_number, outcome, summary, occurred_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(event_id)
    .bind(event_fingerprint)
    .bind(run_id)
    .bind(host_session_id)
    .bind(event_type.as_str())
    .bind(source.as_str())
    .bind(issue_number)
    .bind(outcome.map(AgentRunOutcome::as_str))
    .bind(summary)
    .bind(occurred_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn agent_run_from_row(row: &SqliteRow) -> Result<AgentRun, DaemonError> {
    let host: String = row.try_get("host")?;
    let kind: String = row.try_get("kind")?;
    let phase: String = row.try_get("phase")?;
    let outcome: Option<String> = row.try_get("outcome")?;
    Ok(AgentRun {
        run_id: row.try_get("run_id")?,
        project_id: row.try_get("project_id")?,
        issue_number: row.try_get("issue_number")?,
        host: AgentRunHost::from_db(&host)?,
        host_run_key: row.try_get("host_run_key")?,
        host_session_id: row.try_get("host_session_id")?,
        parent_run_id: row.try_get("parent_run_id")?,
        kind: AgentRunKind::from_db(&kind)?,
        phase: AgentRunPhase::from_db(&phase)?,
        outcome: outcome
            .as_deref()
            .map(AgentRunOutcome::from_db)
            .transpose()?,
        end_reason: row.try_get("end_reason")?,
        display_label: row.try_get("display_label")?,
        summary: row.try_get("summary")?,
        revision: row.try_get("revision")?,
        started_at: row.try_get("started_at")?,
        last_seen_at: row.try_get("last_seen_at")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        ended_at: row.try_get("ended_at")?,
    })
}

fn run_conflict(message: impl Into<String>) -> DaemonError {
    DaemonError::State {
        code: "agent_run_conflict",
        message: message.into(),
    }
}

fn corrupt_run(message: impl Into<String>) -> DaemonError {
    DaemonError::State {
        code: "agent_run_corrupt",
        message: message.into(),
    }
}

pub(crate) fn project_issue_board(
    project_id: &str,
    resources: &[SourceResource],
    runs: &[AgentRun],
    now: &str,
) -> (Vec<IssueBoardCard>, Vec<IssueBoardDiagnostic>) {
    let mut issues_by_number = BTreeMap::<i64, Vec<ParsedIssue>>::new();
    let mut diagnostics = Vec::new();

    for resource in resources {
        if resource.project_id != project_id
            || resource.scope != SourceScope::Project
            || resource.kind != MemoryKind::Memory
        {
            continue;
        }
        if !resource.path.starts_with("issues/") {
            continue;
        }
        match parse_issue(resource) {
            Ok((issue, issue_diagnostics)) => {
                diagnostics.extend(issue_diagnostics);
                issues_by_number
                    .entry(issue.issue_number)
                    .or_default()
                    .push(issue);
            }
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }

    let mut cards = Vec::new();
    for (issue_number, mut issues) in issues_by_number {
        issues.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.resource_id.cmp(&right.resource_id))
        });
        if issues.len() > 1 {
            for issue in issues {
                diagnostics.push(IssueBoardDiagnostic {
                    resource_id: issue.resource_id,
                    path: issue.path,
                    code: IssueBoardDiagnosticCode::DuplicateIssueNumber,
                    message: format!(
                        "ISSUE-{issue_number:03} is declared by more than one effective resource"
                    ),
                });
            }
            continue;
        }
        let issue = issues.pop().expect("one parsed Issue");
        let issue_runs = runs
            .iter()
            .filter(|run| {
                run.project_id == project_id && run.issue_number == Some(issue.issue_number)
            })
            .cloned()
            .collect::<Vec<_>>();
        let projection = project_runs(&issue_runs, now);
        cards.push(issue.into_card(project_id, projection));
    }
    cards.sort_by_key(|issue| issue.issue_number);
    diagnostics.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.resource_id.cmp(&right.resource_id))
            .then_with(|| diagnostic_rank(left.code).cmp(&diagnostic_rank(right.code)))
    });
    (cards, diagnostics)
}

pub(crate) fn project_runs(runs: &[AgentRun], now: &str) -> IssueRunProjection {
    let mut projected_runs = runs
        .iter()
        .cloned()
        .map(|run| project_agent_run(run, now))
        .collect::<Vec<_>>();
    projected_runs.sort_by(latest_run_order);

    let active_runs = projected_runs
        .iter()
        .filter(|run| run.phase == AgentRunPhase::Running)
        .cloned()
        .collect::<Vec<_>>();
    let latest_run = projected_runs.first().cloned();
    IssueRunProjection {
        active_runs,
        latest_run,
    }
}

pub(crate) fn project_agent_run(mut run: AgentRun, now: &str) -> AgentRun {
    if run.phase == AgentRunPhase::Running && run.lease_expires_at.as_str() <= now {
        run.phase = AgentRunPhase::Ended;
        run.outcome = Some(AgentRunOutcome::Unknown);
        run.end_reason = Some(LEASE_EXPIRED_REASON.to_owned());
        run.ended_at = Some(run.lease_expires_at.clone());
    }
    run
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ParsedIssue {
    issue_number: i64,
    issue_key: String,
    resource_id: String,
    path: String,
    lifecycle: IssueLifecycle,
    title: String,
    found_at: Option<String>,
    content_hash: String,
    source_commit_id: Option<String>,
    draft_id: Option<String>,
    draft_revision: Option<String>,
}

impl ParsedIssue {
    fn into_card(self, project_id: &str, projection: IssueRunProjection) -> IssueBoardCard {
        let issue_id = self.resource_id.clone();
        IssueBoardCard {
            issue_id,
            project_id: project_id.to_owned(),
            issue_number: self.issue_number,
            issue_key: self.issue_key,
            resource_id: self.resource_id,
            path: self.path,
            lifecycle: self.lifecycle,
            title: self.title,
            description: String::new(),
            description_excerpt: String::new(),
            external_references: Vec::new(),
            created_at: self.found_at.clone(),
            found_at: self.found_at,
            started_at: None,
            closed_at: None,
            archived_at: None,
            content_hash: self.content_hash,
            source_commit_id: self.source_commit_id,
            draft_id: self.draft_id,
            draft_revision: self.draft_revision,
            board_state: if self.lifecycle == IssueLifecycle::Closed {
                IssueBoardState::Done
            } else {
                IssueBoardState::Todo
            },
            state_revision: 0,
            state_updated_at: None,
            closure_summary: None,
            assignee: None,
            is_stale: false,
            blocked: false,
            blocking_reasons: Vec::new(),
            dependencies: Vec::new(),
            blocking_facts: Vec::new(),
            active_runs: projection.active_runs,
            latest_run: projection.latest_run,
            changed_by_run_id: None,
            verification_level: VerificationLevel::AgentSelf,
            verification_steps: Vec::new(),
            state_events: Vec::new(),
        }
    }
}

fn parse_issue(
    resource: &SourceResource,
) -> Result<(ParsedIssue, Vec<IssueBoardDiagnostic>), IssueBoardDiagnostic> {
    let (lifecycle, digits) =
        parse_issue_path(&resource.path).ok_or_else(|| IssueBoardDiagnostic {
            resource_id: resource.resource_id.clone(),
            path: resource.path.clone(),
            code: IssueBoardDiagnosticCode::MalformedPath,
            message: "Issue path must match issues/open|closed/NNN_lower_snake_case_title.md"
                .to_owned(),
        })?;
    let issue_number = digits.parse::<i64>().map_err(|_| IssueBoardDiagnostic {
        resource_id: resource.resource_id.clone(),
        path: resource.path.clone(),
        code: IssueBoardDiagnosticCode::MalformedPath,
        message: "Issue path contains an invalid Issue number".to_owned(),
    })?;
    let issue_key = format!("ISSUE-{digits}");
    let mut diagnostics = Vec::new();
    let (title, title_number) = match parse_h1(&resource.content) {
        Some(value) => value,
        None => {
            diagnostics.push(IssueBoardDiagnostic {
                resource_id: resource.resource_id.clone(),
                path: resource.path.clone(),
                code: IssueBoardDiagnosticCode::MalformedTitle,
                message: format!("Issue must have an H1 beginning with {issue_key}:"),
            });
            (fallback_title(resource, &issue_key), None)
        }
    };
    if let Some(title_number) = title_number
        && title_number != issue_number
    {
        diagnostics.push(IssueBoardDiagnostic {
            resource_id: resource.resource_id.clone(),
            path: resource.path.clone(),
            code: IssueBoardDiagnosticCode::TitleNumberMismatch,
            message: format!(
                "Issue title declares ISSUE-{title_number:03}, but the path declares {issue_key}"
            ),
        });
    }
    let metadata = parse_metadata(&resource.content);
    Ok((
        ParsedIssue {
            issue_number,
            issue_key,
            resource_id: resource.resource_id.clone(),
            path: resource.path.clone(),
            lifecycle,
            title,
            found_at: metadata.found_at,
            content_hash: resource.content_hash.clone(),
            source_commit_id: resource.source_commit_id.clone(),
            draft_id: resource.draft_id.clone(),
            draft_revision: resource.draft_revision.clone(),
        },
        diagnostics,
    ))
}

fn parse_issue_path(path: &str) -> Option<(IssueLifecycle, &str)> {
    let rest = path.strip_prefix("issues/")?;
    let (lifecycle, filename) = if let Some(filename) = rest.strip_prefix("open/") {
        (IssueLifecycle::Open, filename)
    } else {
        let filename = rest.strip_prefix("closed/")?;
        (IssueLifecycle::Closed, filename)
    };
    if filename.contains('/') || !filename.ends_with(".md") {
        return None;
    }
    let stem = filename.strip_suffix(".md")?;
    let (digits, slug) = stem.split_once('_')?;
    if digits.len() != 3
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || digits.bytes().all(|byte| byte == b'0')
        || !is_lower_snake_case(slug)
    {
        return None;
    }
    Some((lifecycle, digits))
}

fn is_lower_snake_case(value: &str) -> bool {
    value.split('_').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    })
}

fn parse_h1(content: &str) -> Option<(String, Option<i64>)> {
    let h1 = content
        .lines()
        .find_map(|line| line.trim().strip_prefix("# "))?;
    let separator = h1.find([':', '：'])?;
    let key = h1[..separator].trim();
    let digits = key.strip_prefix("ISSUE-")?;
    if digits.len() != 3
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || digits.bytes().all(|byte| byte == b'0')
    {
        return None;
    }
    let title = h1[separator + h1[separator..].chars().next()?.len_utf8()..].trim();
    if title.is_empty() {
        return None;
    }
    Some((title.to_owned(), digits.parse().ok()))
}

fn fallback_title(resource: &SourceResource, issue_key: &str) -> String {
    let title = resource.title.trim();
    if !title.is_empty() && !title.eq_ignore_ascii_case(issue_key) {
        return title.to_owned();
    }
    resource
        .path
        .rsplit('/')
        .next()
        .and_then(|name| name.strip_suffix(".md"))
        .and_then(|stem| stem.split_once('_').map(|(_, slug)| slug))
        .unwrap_or(issue_key)
        .replace('_', " ")
}

#[derive(Default)]
struct IssueMetadata {
    found_at: Option<String>,
}

fn parse_metadata(content: &str) -> IssueMetadata {
    let mut metadata = IssueMetadata::default();
    for line in content.lines() {
        let line = line.trim();
        if !line.starts_with('|') || !line.ends_with('|') {
            continue;
        }
        let cells = line[1..line.len() - 1]
            .split('|')
            .map(str::trim)
            .collect::<Vec<_>>();
        if cells.len() < 2 || is_table_separator(cells[0]) {
            continue;
        }
        let key = cells[0].trim_matches('`').to_ascii_lowercase();
        let value = cells[1].trim().trim_matches('`').trim();
        if value.is_empty() {
            continue;
        }
        match key.as_str() {
            "found" | "found at" | "found_at" | "发现日期" => {
                metadata.found_at = Some(value.to_owned())
            }
            _ => {}
        }
    }
    metadata
}

fn is_table_separator(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| matches!(byte, b'-' | b':' | b' '))
}

fn latest_run_order(left: &AgentRun, right: &AgentRun) -> Ordering {
    meaningful_timestamp(right)
        .cmp(meaningful_timestamp(left))
        .then_with(|| right.run_id.cmp(&left.run_id))
}

fn meaningful_timestamp(run: &AgentRun) -> &str {
    run.ended_at.as_deref().unwrap_or(&run.last_seen_at)
}

fn diagnostic_rank(code: IssueBoardDiagnosticCode) -> u8 {
    match code {
        IssueBoardDiagnosticCode::MalformedPath => 0,
        IssueBoardDiagnosticCode::MalformedTitle => 1,
        IssueBoardDiagnosticCode::TitleNumberMismatch => 2,
        IssueBoardDiagnosticCode::DuplicateIssueNumber => 3,
    }
}

#[cfg(test)]
mod tests {
    use sqlx::Row;
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;

    fn resource(path: &str, content: &str) -> SourceResource {
        SourceResource {
            resource_id: format!("ctx_{path}"),
            project_id: "project-1".to_owned(),
            scope: SourceScope::Project,
            kind: MemoryKind::Memory,
            memory_system: crate::search::MemorySystem::Semantic,
            authority: crate::search::MemoryAuthority::Organization,
            path: path.to_owned(),
            title: "Fallback title".to_owned(),
            description: String::new(),
            content: content.to_owned(),
            content_hash: format!("hash:{path}"),
            source_commit_id: Some("commit-1".to_owned()),
            draft_id: None,
            draft_revision: None,
            episode_id: None,
            run_id: None,
            activity_at: None,
            evidence_hash: None,
        }
    }

    fn external_reference(
        kind: IssueExternalReferenceKind,
        url: impl Into<String>,
    ) -> IssueExternalReference {
        IssueExternalReference {
            kind,
            url: url.into(),
        }
    }

    fn run(
        run_id: &str,
        phase: AgentRunPhase,
        outcome: Option<AgentRunOutcome>,
        last_seen_at: &str,
        lease_expires_at: &str,
    ) -> AgentRun {
        AgentRun {
            run_id: run_id.to_owned(),
            project_id: "project-1".to_owned(),
            issue_number: Some(3),
            host: AgentRunHost::Codex,
            host_run_key: format!("root:{run_id}"),
            host_session_id: Some("session-1".to_owned()),
            parent_run_id: None,
            kind: AgentRunKind::Root,
            phase,
            outcome,
            end_reason: None,
            display_label: None,
            summary: None,
            revision: 1,
            started_at: "2026-08-06T08:00:00.000Z".to_owned(),
            last_seen_at: last_seen_at.to_owned(),
            lease_expires_at: lease_expires_at.to_owned(),
            ended_at: (phase == AgentRunPhase::Ended).then(|| last_seen_at.to_owned()),
        }
    }

    async fn run_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        migrate(&pool).await.unwrap();
        pool
    }

    async fn native_issue(pool: &SqlitePool, project_id: &str, issue_number: i64) {
        sqlx::query(
            "INSERT INTO native_issues (
                issue_id, project_id, issue_number, title, description,
                acceptance_criteria_json, status, revision,
                created_at, updated_at
             ) VALUES ($1, $2, $3, $4, 'Description', '[]', 'todo', 1,
                       '2026-08-06T00:00:00.000Z', '2026-08-06T00:00:00.000Z')",
        )
        .bind(format!("issue_{}", Uuid::new_v4().simple()))
        .bind(project_id)
        .bind(issue_number)
        .bind(format!("Issue {issue_number}"))
        .execute(pool)
        .await
        .unwrap();
    }

    fn lifecycle_request(
        event_id: &str,
        host_run_key: Option<&str>,
        event_type: AgentRunEventType,
    ) -> RecordAgentRunEventRequest {
        RecordAgentRunEventRequest {
            event_id: event_id.to_owned(),
            project_id: "project-1".to_owned(),
            host: AgentRunHost::Codex,
            host_run_key: host_run_key.map(str::to_owned),
            event_type,
            source: AgentRunEventSource::Hook,
            host_session_id: Some("session-1".to_owned()),
            parent_run_id: None,
            parent_host_run_key: None,
            kind: if event_type == AgentRunEventType::SessionEnded {
                None
            } else {
                Some(AgentRunKind::Root)
            },
            issue_key: None,
            outcome: None,
            display_label: None,
            summary: None,
            occurred_at: Some("2026-08-06T08:00:00Z".to_owned()),
        }
    }

    async fn session_hook_start(
        pool: &SqlitePool,
        event_id: &str,
        host_run_key: &str,
        session_id: &str,
    ) -> AgentRun {
        let mut request =
            lifecycle_request(event_id, Some(host_run_key), AgentRunEventType::Started);
        request.host_session_id = Some(session_id.to_owned());
        record_agent_run_event(pool, request)
            .await
            .unwrap()
            .run
            .unwrap()
    }

    #[test]
    fn wire_values_match_swift_and_zig_contracts() {
        assert_eq!(
            serde_json::to_value(AgentRunHost::ClaudeCode).unwrap(),
            "claude-code"
        );
        assert_eq!(
            serde_json::to_value(AgentRunEventType::SessionEnded).unwrap(),
            "session_ended"
        );
        assert_eq!(
            serde_json::to_value(external_reference(
                IssueExternalReferenceKind::PullRequest,
                "https://github.com/acme/widgets/pull/42",
            ))
            .unwrap(),
            serde_json::json!({
                "kind": "pull_request",
                "url": "https://github.com/acme/widgets/pull/42"
            })
        );
        let request: RecordAgentRunEventRequest = serde_json::from_value(serde_json::json!({
            "event_id": "hook_1",
            "project_id": "project-1",
            "host": "codex",
            "host_run_key": "subagent:session-1:agent-1",
            "event_type": "started",
            "source": "hook",
            "host_session_id": "session-1",
            "parent_run_id": null,
            "parent_host_run_key": "root:turn-1",
            "kind": "subagent",
            "issue_key": null,
            "outcome": null,
            "display_label": null,
            "summary": null,
            "occurred_at": null
        }))
        .unwrap();
        assert_eq!(request.parent_host_run_key.as_deref(), Some("root:turn-1"));
        let create: CreateIssueRequest = serde_json::from_value(serde_json::json!({
            "project_id": "project-1",
            "title": "Compatible create",
            "description": "Older callers can omit optional arrays"
        }))
        .unwrap();
        assert!(create.external_references.is_empty());
        let update: UpdateIssueRequest = serde_json::from_value(serde_json::json!({
            "project_id": "project-1",
            "issue_key": "ISSUE-007",
            "expected_revision": 2,
            "external_references": []
        }))
        .unwrap();
        assert_eq!(update.external_references, Some(Vec::new()));
        assert_eq!(
            serde_json::to_value(IssueDetailRequest {
                project_id: "project-1".to_owned(),
                issue_number: 7,
            })
            .unwrap(),
            serde_json::json!({ "project_id": "project-1", "issue_number": 7 })
        );
        assert_eq!(
            serde_json::to_value(GetIssueRequest {
                issue_id: Some("issue_0123456789abcdef0123456789abcdef".to_owned()),
                issue_key: None,
                project_id: None,
            })
            .unwrap(),
            serde_json::json!({
                "issue_id": "issue_0123456789abcdef0123456789abcdef"
            })
        );
    }

    #[tokio::test]
    async fn migration_creates_run_event_tables_and_indexes() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        migrate(&pool).await.unwrap();
        let tables = sqlx::query(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'agent_run%' ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get::<String, _>("name"))
        .collect::<Vec<_>>();
        assert_eq!(tables, ["agent_run_events", "agent_runs"]);

        let event_columns = sqlx::query("PRAGMA table_info(agent_run_events)")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect::<Vec<_>>();
        assert!(event_columns.iter().any(|name| name == "event_fingerprint"));

        let indexes = sqlx::query(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name LIKE 'idx_agent_run%' ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get::<String, _>("name"))
        .collect::<Vec<_>>();
        assert_eq!(
            indexes,
            [
                "idx_agent_run_events_run_occurred",
                "idx_agent_runs_project_issue_latest",
                "idx_agent_runs_project_session",
                "idx_agent_runs_running_lease",
            ]
        );
    }

    #[tokio::test]
    async fn lifecycle_events_are_idempotent_and_children_inherit_issue_and_parent() {
        let pool = run_pool().await;
        let mut root_request = lifecycle_request(
            "hook_root_start",
            Some("root:turn-1"),
            AgentRunEventType::Started,
        );
        root_request.issue_key = Some("ISSUE-003".to_owned());
        let root = record_agent_run_event(&pool, root_request.clone())
            .await
            .unwrap()
            .run
            .unwrap();
        assert_eq!(root.issue_number, Some(3));

        let duplicate = record_agent_run_event(&pool, root_request).await.unwrap();
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.run.unwrap().run_id, root.run_id);

        let mut child_request = lifecycle_request(
            "hook_child_start",
            Some("subagent:session-1:agent-1"),
            AgentRunEventType::Started,
        );
        child_request.kind = Some(AgentRunKind::Subagent);
        child_request.parent_host_run_key = Some("root:turn-1".to_owned());
        let child = record_agent_run_event(&pool, child_request)
            .await
            .unwrap()
            .run
            .unwrap();
        assert_eq!(child.parent_run_id.as_deref(), Some(root.run_id.as_str()));
        assert_eq!(child.issue_number, Some(3));

        let mut session_end =
            lifecycle_request("hook_session_end", None, AgentRunEventType::SessionEnded);
        session_end.outcome = Some(AgentRunOutcome::Unknown);
        let ended = record_agent_run_event(&pool, session_end).await.unwrap();
        assert!(ended.run.is_none());
        assert_eq!(ended.affected_runs.len(), 2);
        assert!(ended.affected_runs.iter().all(|run| {
            run.phase == AgentRunPhase::Ended
                && run.outcome == Some(AgentRunOutcome::Unknown)
                && run.end_reason.as_deref() == Some(SESSION_ENDED_REASON)
        }));
    }

    #[tokio::test]
    async fn new_hook_root_turn_recovers_previous_bound_root_and_can_take_over_issue() {
        let pool = run_pool().await;
        native_issue(&pool, "project-1", 3).await;

        let first = record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_turn_one_start",
                Some("root:turn-one"),
                AgentRunEventType::Started,
            ),
        )
        .await
        .unwrap()
        .run
        .unwrap();
        let first = start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(first.run_id),
                issue_key: "ISSUE-003".to_owned(),
                expected_revision: Some(first.revision),
                session_id: None,
            },
        )
        .await
        .unwrap()
        .run
        .unwrap();

        let second_request = lifecycle_request(
            "hook_turn_two_start",
            Some("root:turn-two"),
            AgentRunEventType::Started,
        );
        let second_response = record_agent_run_event(&pool, second_request.clone())
            .await
            .unwrap();
        let second = second_response.run.unwrap();
        assert_eq!(second.phase, AgentRunPhase::Running);
        assert_eq!(second.issue_number, None);
        assert_eq!(second_response.affected_runs.len(), 2);
        let recovered = &second_response.affected_runs[0];
        assert_eq!(recovered.run_id, first.run_id);
        assert_eq!(recovered.phase, AgentRunPhase::Ended);
        assert_eq!(recovered.outcome, Some(AgentRunOutcome::Unknown));
        assert_eq!(recovered.end_reason.as_deref(), Some(RECOVERED_END_REASON));
        assert_eq!(recovered.revision, first.revision + 1);
        assert_eq!(second_response.affected_runs[1].run_id, second.run_id);

        let duplicate = record_agent_run_event(&pool, second_request).await.unwrap();
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.affected_runs.len(), 1);
        assert_eq!(duplicate.affected_runs[0].run_id, second.run_id);
        assert_eq!(
            load_agent_run(&pool, &first.run_id)
                .await
                .unwrap()
                .unwrap()
                .revision,
            recovered.revision
        );
        let recovery_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM agent_run_events
             WHERE run_id = $1 AND event_type = 'ended' AND source = 'recovery'",
        )
        .bind(&first.run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(recovery_events, 1);

        let taken_over = start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(second.run_id.clone()),
                issue_key: "ISSUE-003".to_owned(),
                expected_revision: Some(second.revision),
                session_id: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(taken_over.board_state, IssueBoardState::InProgress);
        assert_eq!(
            taken_over.run.as_ref().map(|run| run.run_id.as_str()),
            Some(second.run_id.as_str())
        );
        assert_eq!(taken_over.run.unwrap().issue_number, Some(3));
    }

    #[tokio::test]
    async fn same_key_subagent_and_mcp_manual_starts_do_not_recover_root_runs() {
        let pool = run_pool().await;
        let root_request = lifecycle_request(
            "hook_same_key_start",
            Some("root:same-key"),
            AgentRunEventType::Started,
        );
        let root = record_agent_run_event(&pool, root_request)
            .await
            .unwrap()
            .run
            .unwrap();

        let same_key = record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_same_key_heartbeat",
                Some("root:same-key"),
                AgentRunEventType::Started,
            ),
        )
        .await
        .unwrap();
        assert_eq!(same_key.affected_runs.len(), 1);
        assert_eq!(same_key.run.as_ref().unwrap().run_id, root.run_id);
        assert_eq!(same_key.run.as_ref().unwrap().phase, AgentRunPhase::Running);

        let mut child_request = lifecycle_request(
            "hook_child_new_key",
            Some("subagent:session-1:new-child"),
            AgentRunEventType::Started,
        );
        child_request.kind = Some(AgentRunKind::Subagent);
        child_request.parent_host_run_key = Some("root:same-key".to_owned());
        let child = record_agent_run_event(&pool, child_request).await.unwrap();
        assert_eq!(child.affected_runs.len(), 1);
        assert_eq!(
            load_agent_run(&pool, &root.run_id)
                .await
                .unwrap()
                .unwrap()
                .phase,
            AgentRunPhase::Running
        );

        let mut first_manual = lifecycle_request(
            "mcp_manual_one_start",
            Some("manual:turn-one"),
            AgentRunEventType::Started,
        );
        first_manual.host = AgentRunHost::Manual;
        first_manual.source = AgentRunEventSource::Mcp;
        first_manual.host_session_id = Some("manual-session".to_owned());
        let first_manual = record_agent_run_event(&pool, first_manual)
            .await
            .unwrap()
            .run
            .unwrap();
        let mut second_manual = lifecycle_request(
            "mcp_manual_two_start",
            Some("manual:turn-two"),
            AgentRunEventType::Started,
        );
        second_manual.host = AgentRunHost::Manual;
        second_manual.source = AgentRunEventSource::Mcp;
        second_manual.host_session_id = Some("manual-session".to_owned());
        let second_manual = record_agent_run_event(&pool, second_manual).await.unwrap();
        assert_eq!(second_manual.affected_runs.len(), 1);
        assert_eq!(
            load_agent_run(&pool, &first_manual.run_id)
                .await
                .unwrap()
                .unwrap()
                .phase,
            AgentRunPhase::Running
        );
    }

    #[tokio::test]
    async fn event_id_requires_an_identical_full_request_fingerprint() {
        let pool = run_pool().await;
        let request = lifecycle_request(
            "hook_fingerprint_collision",
            Some("root:fingerprint-turn"),
            AgentRunEventType::Started,
        );
        record_agent_run_event(&pool, request.clone())
            .await
            .unwrap();

        let duplicate = record_agent_run_event(&pool, request.clone())
            .await
            .unwrap();
        assert!(duplicate.duplicate);
        let fingerprint: String = sqlx::query_scalar(
            "SELECT event_fingerprint FROM agent_run_events WHERE event_id = $1",
        )
        .bind(&request.event_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(fingerprint.starts_with("sha256:"));
        assert_eq!(fingerprint.len(), "sha256:".len() + 64);

        let mut collision = request;
        collision.display_label = Some("same id, different request".to_owned());
        let error = record_agent_run_event(&pool, collision).await.unwrap_err();
        assert!(matches!(
            error,
            DaemonError::State {
                code: "agent_run_conflict",
                ..
            }
        ));
        let event_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM agent_run_events WHERE event_id = 'hook_fingerprint_collision'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(event_count, 1);
    }

    #[tokio::test]
    async fn repeated_session_end_closes_runs_observed_after_the_first_application() {
        let pool = run_pool().await;
        record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_first_start",
                Some("root:first-turn"),
                AgentRunEventType::Started,
            ),
        )
        .await
        .unwrap();
        let session_end = lifecycle_request(
            "hook_repeatable_session_end",
            None,
            AgentRunEventType::SessionEnded,
        );
        let first = record_agent_run_event(&pool, session_end.clone())
            .await
            .unwrap();
        assert!(!first.duplicate);
        assert_eq!(first.affected_runs.len(), 1);

        let late_run = record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_late_start",
                Some("root:late-turn"),
                AgentRunEventType::Started,
            ),
        )
        .await
        .unwrap()
        .run
        .unwrap();
        assert_eq!(late_run.phase, AgentRunPhase::Running);

        let repeated = record_agent_run_event(&pool, session_end).await.unwrap();
        assert!(repeated.duplicate);
        assert_eq!(repeated.affected_runs.len(), 1);
        assert_eq!(repeated.affected_runs[0].run_id, late_run.run_id);
        assert_eq!(repeated.affected_runs[0].phase, AgentRunPhase::Ended);
        let event_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM agent_run_events WHERE event_id = 'hook_repeatable_session_end'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(event_count, 1);
    }

    #[tokio::test]
    async fn normal_stop_ends_the_run_without_inferring_an_outcome() {
        let pool = run_pool().await;
        let started = record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_start",
                Some("root:turn-1"),
                AgentRunEventType::Started,
            ),
        )
        .await
        .unwrap()
        .run
        .unwrap();
        let stop = lifecycle_request("hook_stop", Some("root:turn-1"), AgentRunEventType::Ended);
        let after_stop = record_agent_run_event(&pool, stop)
            .await
            .unwrap()
            .run
            .unwrap();
        assert_eq!(after_stop.phase, AgentRunPhase::Ended);
        assert_eq!(after_stop.outcome, None);
        assert_eq!(after_stop.end_reason.as_deref(), Some(HOOK_END_REASON));
        assert_eq!(after_stop.summary, None);
        assert_eq!(after_stop.run_id, started.run_id);
    }

    #[tokio::test]
    async fn dsh_hook_run_can_bind_and_release_an_issue() {
        let pool = run_pool().await;
        native_issue(&pool, "project-1", 3).await;
        let request =
            |event_id: &str, host_run_key: Option<&str>, event_type: AgentRunEventType| {
                RecordAgentRunEventRequest {
                    event_id: event_id.to_owned(),
                    project_id: "project-1".to_owned(),
                    host: AgentRunHost::Dsh,
                    host_run_key: host_run_key.map(str::to_owned),
                    event_type,
                    source: AgentRunEventSource::Hook,
                    host_session_id: Some("ds-session-1".to_owned()),
                    parent_run_id: None,
                    parent_host_run_key: None,
                    kind: if event_type == AgentRunEventType::SessionEnded {
                        None
                    } else {
                        Some(AgentRunKind::Root)
                    },
                    issue_key: None,
                    outcome: None,
                    display_label: None,
                    summary: None,
                    occurred_at: None,
                }
            };
        let started = record_agent_run_event(
            &pool,
            request("dsh_start", Some("root:turn-1"), AgentRunEventType::Started),
        )
        .await
        .unwrap()
        .run
        .unwrap();
        assert_eq!(started.host, AgentRunHost::Dsh);

        let claimed = start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(started.run_id.clone()),
                issue_key: "ISSUE-003".to_owned(),
                expected_revision: Some(started.revision),
                session_id: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(claimed.board_state, IssueBoardState::InProgress);

        let after_stop = record_agent_run_event(
            &pool,
            request("dsh_stop", Some("root:turn-1"), AgentRunEventType::Ended),
        )
        .await
        .unwrap()
        .run
        .unwrap();
        assert_eq!(after_stop.phase, AgentRunPhase::Ended);
        assert_eq!(after_stop.run_id, started.run_id);
    }

    #[tokio::test]
    async fn end_without_start_is_retained_as_a_recovered_run() {
        let pool = run_pool().await;
        let stop = lifecycle_request(
            "hook_stop_without_start",
            Some("root:turn-2"),
            AgentRunEventType::Ended,
        );
        let run = record_agent_run_event(&pool, stop)
            .await
            .unwrap()
            .run
            .unwrap();
        assert_eq!(run.phase, AgentRunPhase::Ended);
        assert_eq!(run.outcome, None);
        assert_eq!(run.end_reason.as_deref(), Some(RECOVERED_END_REASON));
    }

    #[tokio::test]
    async fn starting_issue_work_is_cas_guarded_idempotent_and_cannot_rebind() {
        let pool = run_pool().await;
        native_issue(&pool, "project-1", 3).await;
        native_issue(&pool, "project-1", 4).await;
        let started = record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_start_for_bind",
                Some("root:turn-3"),
                AgentRunEventType::Started,
            ),
        )
        .await
        .unwrap()
        .run
        .unwrap();
        let mut child_request = lifecycle_request(
            "hook_child_before_bind",
            Some("subagent:session-1:agent-before-bind"),
            AgentRunEventType::Started,
        );
        child_request.kind = Some(AgentRunKind::Subagent);
        child_request.parent_host_run_key = Some("root:turn-3".to_owned());
        let child = record_agent_run_event(&pool, child_request)
            .await
            .unwrap()
            .run
            .unwrap();
        assert_eq!(child.issue_number, None);
        let request = StartIssueWorkRequest {
            project_id: "project-1".to_owned(),
            run_id: Some(started.run_id.clone()),
            issue_key: "ISSUE-003".to_owned(),
            expected_revision: Some(started.revision),
            session_id: None,
        };
        let started_work = start_issue_work(&pool, request.clone()).await.unwrap();
        assert_eq!(started_work.issue_key, "ISSUE-003");
        assert_eq!(started_work.board_state, IssueBoardState::InProgress);
        assert_eq!(started_work.run.as_ref().unwrap().issue_number, Some(3));
        assert_eq!(
            load_agent_run(&pool, &child.run_id)
                .await
                .unwrap()
                .unwrap()
                .issue_number,
            Some(3)
        );

        let repeated = start_issue_work(&pool, request).await.unwrap();
        assert_eq!(repeated.state_revision, started_work.state_revision);
        assert_eq!(
            repeated.run.as_ref().unwrap().revision,
            started_work.run.as_ref().unwrap().revision
        );

        let error = start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(started.run_id),
                issue_key: "ISSUE-004".to_owned(),
                expected_revision: Some(started_work.run.as_ref().unwrap().revision),
                session_id: None,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            DaemonError::State {
                code: "agent_run_conflict",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn native_issue_create_and_update_keep_content_separate_from_status() {
        let pool = run_pool().await;
        let created = create_issue(
            &pool,
            CreateIssueRequest {
                project_id: "project-1".to_owned(),
                title: "Export native Issues as Markdown".to_owned(),
                description: "Provide an explicit export operation.".to_owned(),
                acceptance_criteria: vec!["Export preserves stable Issue keys".to_owned()],
                external_references: Vec::new(),
                dependencies: Vec::new(),
                blocking_facts: Vec::new(),
                verification_level: VerificationLevel::AgentSelf,
                verification_steps: Vec::new(),
            },
        )
        .await
        .unwrap();
        assert_eq!(created.issue_key, "ISSUE-001");
        assert_eq!(created.board_state, IssueBoardState::Todo);
        assert!(validate_issue_id(&created.issue_id).is_ok());
        assert_eq!(
            resolve_native_issue_identity(&pool, &created.issue_id)
                .await
                .unwrap(),
            Some(("project-1".to_owned(), 1))
        );

        let updated = update_issue(
            &pool,
            UpdateIssueRequest {
                project_id: "project-1".to_owned(),
                issue_key: created.issue_key,
                expected_revision: created.revision,
                title: Some("Export structured Issues as Markdown".to_owned()),
                description: None,
                acceptance_criteria: None,
                external_references: None,
                dependencies: None,
                blocking_facts: None,
                verification_level: None,
                verification_steps: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(updated.board_state, IssueBoardState::Todo);
        assert_eq!(updated.revision, 2);

        let cards = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T01:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        assert_eq!(cards[0].title, "Export structured Issues as Markdown");
        assert_eq!(
            cards[0].description,
            "Provide an explicit export operation."
        );
        assert_eq!(cards[0].issue_id, created.issue_id);
        assert!(cards[0].path.is_empty());
        assert!(cards[0].draft_id.is_none());
    }

    #[tokio::test]
    async fn ensure_native_issue_in_project_confirms_existence_and_missing() {
        let pool = run_pool().await;
        let created = create_issue(
            &pool,
            CreateIssueRequest {
                project_id: "project-1".to_owned(),
                title: "Issue for key lookup".to_owned(),
                description: "body".to_owned(),
                acceptance_criteria: vec![],
                external_references: Vec::new(),
                dependencies: Vec::new(),
                blocking_facts: Vec::new(),
                verification_level: VerificationLevel::AgentSelf,
                verification_steps: Vec::new(),
            },
        )
        .await
        .unwrap();
        assert_eq!(created.issue_key, "ISSUE-001");

        assert_eq!(
            ensure_native_issue_in_project(&pool, "project-1", 1)
                .await
                .unwrap(),
            Some(1)
        );
        assert_eq!(
            ensure_native_issue_in_project(&pool, "project-1", 999)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            ensure_native_issue_in_project(&pool, "other-project", 1)
                .await
                .unwrap(),
            None
        );
        assert_eq!(parse_issue_reference("ISSUE-001").unwrap(), 1);
        assert_eq!(parse_issue_reference("ISSUE-026").unwrap(), 26);
        assert!(parse_issue_reference("ISSUE-000").is_err());
        assert!(parse_issue_reference("issue-001").is_err());
    }

    #[tokio::test]
    async fn issue_verification_level_and_steps_round_trip() {
        let pool = run_pool().await;
        let created = create_issue(
            &pool,
            CreateIssueRequest {
                project_id: "project-1".to_owned(),
                title: "Verification protocol".to_owned(),
                description: "Needs human verification.".to_owned(),
                acceptance_criteria: vec![],
                external_references: Vec::new(),
                dependencies: Vec::new(),
                blocking_facts: Vec::new(),
                verification_level: VerificationLevel::HumanRequired,
                verification_steps: vec![
                    VerificationStep {
                        text: "Open Settings > Agent".to_owned(),
                        completed: false,
                    },
                    VerificationStep {
                        text: "Toggle opencode and confirm install".to_owned(),
                        completed: false,
                    },
                ],
            },
        )
        .await
        .unwrap();

        let card = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T01:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        assert_eq!(card[0].verification_level, VerificationLevel::HumanRequired);
        assert_eq!(card[0].verification_steps.len(), 2);

        let updated = update_issue(
            &pool,
            UpdateIssueRequest {
                project_id: "project-1".to_owned(),
                issue_key: created.issue_key,
                expected_revision: created.revision,
                title: None,
                description: None,
                acceptance_criteria: None,
                external_references: None,
                dependencies: None,
                blocking_facts: None,
                verification_level: Some(VerificationLevel::AgentSelf),
                verification_steps: Some(vec![]),
            },
        )
        .await
        .unwrap();
        assert_eq!(updated.board_state, IssueBoardState::Todo);

        let card = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T02:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        assert_eq!(card[0].verification_level, VerificationLevel::AgentSelf);
        assert!(card[0].verification_steps.is_empty());
    }

    #[tokio::test]
    async fn session_single_hold_pause_resume_round_trip() {
        let pool = run_pool().await;
        // Create two Issues.
        let first = create_issue(
            &pool,
            CreateIssueRequest {
                project_id: "project-1".to_owned(),
                title: "First".to_owned(),
                description: "one".to_owned(),
                acceptance_criteria: vec![],
                external_references: Vec::new(),
                dependencies: Vec::new(),
                blocking_facts: Vec::new(),
                verification_level: VerificationLevel::AgentSelf,
                verification_steps: Vec::new(),
            },
        )
        .await
        .unwrap();
        let second = create_issue(
            &pool,
            CreateIssueRequest {
                project_id: "project-1".to_owned(),
                title: "Second".to_owned(),
                description: "two".to_owned(),
                acceptance_criteria: vec![],
                external_references: Vec::new(),
                dependencies: Vec::new(),
                blocking_facts: Vec::new(),
                verification_level: VerificationLevel::AgentSelf,
                verification_steps: Vec::new(),
            },
        )
        .await
        .unwrap();

        // A hook run in session-a starts the first Issue.
        let run_a = session_hook_start(&pool, "session_a_1", "root:a1", "session-a").await;
        let started = start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(run_a.run_id.clone()),
                issue_key: first.issue_key.clone(),
                expected_revision: Some(run_a.revision),
                session_id: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(started.board_state, IssueBoardState::InProgress);

        // A non-Hook run in the same session cannot start the second Issue
        // while the first is In Progress. Hook root turns use rollover
        // recovery; MCP/manual starts deliberately do not.
        let mut run_a2_request =
            lifecycle_request("session_a_2", Some("root:a2"), AgentRunEventType::Started);
        run_a2_request.source = AgentRunEventSource::Mcp;
        run_a2_request.host_session_id = Some("session-a".to_owned());
        let run_a2 = record_agent_run_event(&pool, run_a2_request)
            .await
            .unwrap()
            .run
            .unwrap();
        let conflict = start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(run_a2.run_id.clone()),
                issue_key: second.issue_key.clone(),
                expected_revision: Some(run_a2.revision),
                session_id: None,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(
            conflict,
            DaemonError::State {
                code: "agent_run_conflict",
                ..
            }
        ));

        // A different session can start the second Issue.
        let run_b = session_hook_start(&pool, "session_b_1", "root:b1", "session-b").await;
        let other = start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(run_b.run_id.clone()),
                issue_key: second.issue_key.clone(),
                expected_revision: Some(run_b.revision),
                session_id: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(other.board_state, IssueBoardState::InProgress);

        // Pause the first Issue; then the first session can start a third Issue.
        let third = create_issue(
            &pool,
            CreateIssueRequest {
                project_id: "project-1".to_owned(),
                title: "Third".to_owned(),
                description: "three".to_owned(),
                acceptance_criteria: vec![],
                external_references: Vec::new(),
                dependencies: Vec::new(),
                blocking_facts: Vec::new(),
                verification_level: VerificationLevel::AgentSelf,
                verification_steps: Vec::new(),
            },
        )
        .await
        .unwrap();
        let started_run_id = started.run.as_ref().unwrap().run_id.clone();
        let paused = pause_issue_work(
            &pool,
            PauseIssueRequest {
                project_id: "project-1".to_owned(),
                run_id: started.run.as_ref().unwrap().run_id.clone(),
                issue_key: first.issue_key.clone(),
            },
        )
        .await
        .unwrap();
        assert_eq!(paused.board_state, IssueBoardState::Paused);

        let run_a3 = session_hook_start(&pool, "session_a_3", "root:a3", "session-a").await;
        let started_third = start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(run_a3.run_id.clone()),
                issue_key: third.issue_key.clone(),
                expected_revision: Some(run_a3.revision),
                session_id: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(started_third.board_state, IssueBoardState::InProgress);

        // Pause the third Issue so session-a can resume the first.
        let _ = pause_issue_work(
            &pool,
            PauseIssueRequest {
                project_id: "project-1".to_owned(),
                run_id: started_third.run.as_ref().unwrap().run_id.clone(),
                issue_key: third.issue_key.clone(),
            },
        )
        .await
        .unwrap();

        // Resume the first with its pausing run.
        let resumed = resume_issue_work(
            &pool,
            ResumeIssueRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(started_run_id),
                issue_key: first.issue_key.clone(),
                takeover: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(resumed.board_state, IssueBoardState::InProgress);

        // Pausing a non-In Progress Issue is rejected.
        let pause_todo = pause_issue_work(
            &pool,
            PauseIssueRequest {
                project_id: "project-1".to_owned(),
                run_id: started.run.as_ref().unwrap().run_id.clone(),
                issue_key: second.issue_key.clone(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(pause_todo, DaemonError::State { .. }));
    }

    #[test]
    fn issue_description_excerpt_picks_first_paragraph_and_strips_markdown() {
        assert_eq!(
            issue_description_excerpt("## 背景\n\n这是第一段真实描述。\n\n## 期望\n第二段。"),
            "这是第一段真实描述。"
        );
        assert_eq!(issue_description_excerpt("- 列表项描述"), "列表项描述");
        assert_eq!(issue_description_excerpt("> 引用内容"), "引用内容");
        assert_eq!(issue_description_excerpt(""), "");
        assert_eq!(issue_description_excerpt("   \n\n  "), "");

        // Long description truncates with an ellipsis.
        let long = "a".repeat(300);
        let excerpt = issue_description_excerpt(&long);
        assert!(excerpt.len() <= 241);
        assert!(excerpt.ends_with('…'));
    }

    #[tokio::test]
    async fn issue_board_card_exposes_description_excerpt() {
        let pool = run_pool().await;
        let created = create_issue(
            &pool,
            CreateIssueRequest {
                project_id: "project-1".to_owned(),
                title: "Excerpted".to_owned(),
                description: "## 背景\n\n首段摘要内容。".to_owned(),
                acceptance_criteria: vec![],
                external_references: Vec::new(),
                dependencies: Vec::new(),
                blocking_facts: Vec::new(),
                verification_level: VerificationLevel::AgentSelf,
                verification_steps: Vec::new(),
            },
        )
        .await
        .unwrap();

        let cards = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T01:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        assert_eq!(cards[0].description_excerpt, "首段摘要内容。");
        assert!(cards[0].description.contains("## 背景"));
        assert!(created.issue_id == cards[0].issue_id);
        assert!(cards[0].changed_by_run_id.is_none());

        // After a run starts work, the board exposes who changed it.
        let run =
            session_hook_start(&pool, "excerpt_start", "root:excerpt", "session-excerpt").await;
        let started = start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(run.run_id.clone()),
                issue_key: created.issue_key.clone(),
                expected_revision: Some(run.revision),
                session_id: None,
            },
        )
        .await
        .unwrap();
        let cards = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T02:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        assert_eq!(
            cards[0].changed_by_run_id.as_deref(),
            Some(started.run.as_ref().unwrap().run_id.as_str())
        );

        let server_record = ServerKanbanIssue {
            project_id: "project-1".to_owned(),
            issue_id: cards[0].issue_id.clone(),
            issue_number: cards[0].issue_number,
            assignee: IssueMember {
                user_id: "usr_test".to_owned(),
                email: "member@example.com".to_owned(),
                display_name: Some("Member".to_owned()),
                avatar_url: None,
                role: "member".to_owned(),
            },
            content_revision: cards[0].state_revision,
            payload: ServerKanbanIssueSnapshot {
                issue: cards[0].clone(),
                acceptance_criteria: Vec::new(),
            },
        };
        sync_server_kanban_issues(&pool, "project-1", std::slice::from_ref(&server_record))
            .await
            .unwrap();
        let synced = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T02:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        assert_eq!(
            synced[0].changed_by_run_id.as_deref(),
            Some(started.run.as_ref().unwrap().run_id.as_str())
        );

        let mut foreign_record = server_record;
        foreign_record.payload.issue.changed_by_run_id = Some("run_remote".to_owned());
        sync_server_kanban_issues(&pool, "project-1", &[foreign_record])
            .await
            .unwrap();
        let synced = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T02:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        assert!(synced[0].changed_by_run_id.is_none());
    }

    #[test]
    fn render_issue_markdown_is_deterministic_and_complete() {
        let issue = IssueBoardCard {
            issue_id: "issue_0123456789abcdef0123456789abcdef".to_owned(),
            project_id: "project-1".to_owned(),
            issue_number: 7,
            issue_key: "ISSUE-007".to_owned(),
            resource_id: "issue_0123456789abcdef0123456789abcdef".to_owned(),
            path: String::new(),
            lifecycle: IssueLifecycle::Open,
            title: "Export native Issues".to_owned(),
            description: "body".to_owned(),
            description_excerpt: "body".to_owned(),
            external_references: Vec::new(),
            found_at: Some("2026-08-07T03:00:00Z".to_owned()),
            created_at: Some("2026-08-07T03:00:00Z".to_owned()),
            started_at: Some("2026-08-08T03:00:00Z".to_owned()),
            closed_at: None,
            archived_at: None,
            content_hash: "native:1".to_owned(),
            source_commit_id: None,
            draft_id: None,
            draft_revision: None,
            board_state: IssueBoardState::InProgress,
            state_revision: 1,
            state_updated_at: Some("2026-08-08T03:00:00Z".to_owned()),
            closure_summary: None,
            assignee: None,
            is_stale: false,
            blocked: false,
            blocking_reasons: Vec::new(),
            dependencies: Vec::new(),
            blocking_facts: Vec::new(),
            active_runs: Vec::new(),
            latest_run: None,
            changed_by_run_id: None,
            verification_level: VerificationLevel::AgentSelf,
            verification_steps: Vec::new(),
            state_events: Vec::new(),
        };
        let criteria = vec![
            "Preserve stable keys".to_owned(),
            "Deterministic output".to_owned(),
        ];
        let first = render_issue_markdown(&issue, "## 背景\n\n正文。", &criteria);
        let second = render_issue_markdown(&issue, "## 背景\n\n正文。", &criteria);
        assert_eq!(first, second);
        assert!(first.contains("# Export native Issues"));
        assert!(first.contains("- **Issue**: ISSUE-007"));
        assert!(first.contains("- **Status**: in_progress"));
        assert!(first.contains("## 背景"));
        assert!(first.contains("## 验收标准"));
        assert!(first.contains("- [ ] Preserve stable keys"));
        assert!(first.contains("- [ ] Deterministic output"));
        assert_eq!(issue_export_filename(7), "ISSUE-007.md");
    }

    #[tokio::test]
    async fn native_issue_external_references_round_trip_normalize_and_clear() {
        let pool = run_pool().await;
        let issue_url = "https://github.com/acme/widgets/issues/17?view=full#discussion";
        let pull_request_url = "https://github.com/acme/widgets/pull/42?diff=split#files";
        let created = create_issue(
            &pool,
            CreateIssueRequest {
                project_id: "project-1".to_owned(),
                title: "Link upstream work".to_owned(),
                description: "Keep the native Issue connected to its remote work.".to_owned(),
                acceptance_criteria: Vec::new(),
                external_references: vec![
                    external_reference(
                        IssueExternalReferenceKind::Issue,
                        "  HTTPS://GitHub.COM:443/acme/widgets/issues/17?view=full#discussion  ",
                    ),
                    external_reference(IssueExternalReferenceKind::Issue, issue_url),
                    external_reference(IssueExternalReferenceKind::PullRequest, pull_request_url),
                ],
                dependencies: Vec::new(),
                blocking_facts: Vec::new(),
                verification_level: VerificationLevel::AgentSelf,
                verification_steps: Vec::new(),
            },
        )
        .await
        .unwrap();

        let cards = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T01:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        assert_eq!(
            cards[0].external_references,
            vec![
                external_reference(IssueExternalReferenceKind::Issue, issue_url),
                external_reference(IssueExternalReferenceKind::PullRequest, pull_request_url,),
            ]
        );
        let detail = load_native_issue_detail(
            &pool,
            "project-1",
            1,
            &[],
            "2026-08-06T01:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        assert_eq!(
            detail.issue.external_references,
            cards[0].external_references
        );

        let preserved = update_issue(
            &pool,
            UpdateIssueRequest {
                project_id: "project-1".to_owned(),
                issue_key: created.issue_key.clone(),
                expected_revision: created.revision,
                title: Some("Link upstream Issue and PR".to_owned()),
                description: None,
                acceptance_criteria: None,
                external_references: None,
                dependencies: None,
                blocking_facts: None,
                verification_level: None,
                verification_steps: None,
            },
        )
        .await
        .unwrap();
        let preserved_detail = load_native_issue_detail(
            &pool,
            "project-1",
            1,
            &[],
            "2026-08-06T01:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        assert_eq!(
            preserved_detail.issue.external_references,
            cards[0].external_references
        );

        update_issue(
            &pool,
            UpdateIssueRequest {
                project_id: "project-1".to_owned(),
                issue_key: created.issue_key,
                expected_revision: preserved.revision,
                title: None,
                description: None,
                acceptance_criteria: None,
                external_references: Some(Vec::new()),
                dependencies: None,
                blocking_facts: None,
                verification_level: None,
                verification_steps: None,
            },
        )
        .await
        .unwrap();
        let cleared = load_native_issue_detail(
            &pool,
            "project-1",
            1,
            &[],
            "2026-08-06T01:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        assert!(cleared.issue.external_references.is_empty());
    }

    #[test]
    fn issue_external_reference_validation_rejects_unsafe_or_oversized_values() {
        for reference in [
            external_reference(
                IssueExternalReferenceKind::Issue,
                "file:///tmp/upstream-issue",
            ),
            external_reference(
                IssueExternalReferenceKind::PullRequest,
                "https://user:secret@github.com/acme/widgets/pull/42",
            ),
            external_reference(IssueExternalReferenceKind::Issue, "https://"),
            external_reference(
                IssueExternalReferenceKind::Issue,
                format!("https://example.com/{}", "a".repeat(2_049)),
            ),
        ] {
            assert!(normalize_issue_external_references(&[reference]).is_err());
        }

        let too_many = (0..=MAX_ISSUE_EXTERNAL_REFERENCES)
            .map(|number| {
                external_reference(
                    IssueExternalReferenceKind::Issue,
                    format!("https://example.com/issues/{number}"),
                )
            })
            .collect::<Vec<_>>();
        assert!(normalize_issue_external_references(&too_many).is_err());
    }

    fn blocking_fact(
        fact_id: &str,
        kind: IssueBlockingFactKind,
        description: &str,
        satisfied: bool,
    ) -> IssueBlockingFact {
        IssueBlockingFact {
            fact_id: fact_id.to_owned(),
            kind,
            value: None,
            description: description.to_owned(),
            satisfied,
        }
    }

    async fn approve_done(pool: &SqlitePool, project_id: &str, issue_number: i64, revision: i64) {
        let mut tx = pool.begin().await.unwrap();
        set_native_issue_state_tx(
            &mut tx,
            project_id,
            issue_number,
            IssueBoardState::Done,
            None,
            None,
            "2026-08-06T00:30:00.000Z",
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(revision, 1);
    }

    #[tokio::test]
    async fn issue_dependencies_resolve_blocked_state_and_clear_after_done() {
        let pool = run_pool().await;
        native_issue(&pool, "project-1", 3).await;
        native_issue(&pool, "project-1", 4).await;
        let created = create_issue(
            &pool,
            CreateIssueRequest {
                project_id: "project-1".to_owned(),
                title: "Dependent work".to_owned(),
                description: "Depends on two issues.".to_owned(),
                acceptance_criteria: Vec::new(),
                external_references: Vec::new(),
                dependencies: vec!["ISSUE-003".to_owned(), "ISSUE-004".to_owned()],
                blocking_facts: Vec::new(),
                verification_level: VerificationLevel::AgentSelf,
                verification_steps: Vec::new(),
            },
        )
        .await
        .unwrap();
        assert_eq!(created.issue_key, "ISSUE-001");

        let board = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T01:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        let dependent = board
            .iter()
            .find(|card| card.issue_key == "ISSUE-001")
            .unwrap();
        assert!(dependent.blocked);
        assert_eq!(dependent.dependencies.len(), 2);
        assert!(
            dependent
                .dependencies
                .iter()
                .all(|dependency| { dependency.board_state != IssueBoardState::Done })
        );
        assert_eq!(dependent.blocking_reasons.len(), 2);
        assert!(
            dependent
                .blocking_reasons
                .iter()
                .all(|reason| reason.kind == IssueBlockingReasonKind::Dependency)
        );
        assert!(
            board
                .iter()
                .all(|card| !card.blocked || card.issue_key == "ISSUE-001")
        );

        approve_done(&pool, "project-1", 3, 1).await;
        approve_done(&pool, "project-1", 4, 1).await;
        let board = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T02:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        let dependent = board
            .iter()
            .find(|card| card.issue_key == "ISSUE-001")
            .unwrap();
        assert!(!dependent.blocked);
        assert!(dependent.blocking_reasons.is_empty());
        assert!(
            dependent
                .dependencies
                .iter()
                .all(|dependency| { dependency.board_state == IssueBoardState::Done })
        );
    }

    #[tokio::test]
    async fn issue_dependencies_reject_self_missing_duplicate_and_cycles() {
        let pool = run_pool().await;
        native_issue(&pool, "project-1", 3).await;
        native_issue(&pool, "project-1", 4).await;

        let missing = create_issue(
            &pool,
            CreateIssueRequest {
                project_id: "project-1".to_owned(),
                title: "Missing dependency".to_owned(),
                description: "Dependency does not exist.".to_owned(),
                acceptance_criteria: Vec::new(),
                external_references: Vec::new(),
                dependencies: vec!["ISSUE-007".to_owned()],
                blocking_facts: Vec::new(),
                verification_level: VerificationLevel::AgentSelf,
                verification_steps: Vec::new(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(missing, DaemonError::NotFound(_)));

        let duplicate = create_issue(
            &pool,
            CreateIssueRequest {
                project_id: "project-1".to_owned(),
                title: "Duplicate dependency".to_owned(),
                description: "Repeats a dependency.".to_owned(),
                acceptance_criteria: Vec::new(),
                external_references: Vec::new(),
                dependencies: vec!["ISSUE-003".to_owned(), "ISSUE-003".to_owned()],
                blocking_facts: Vec::new(),
                verification_level: VerificationLevel::AgentSelf,
                verification_steps: Vec::new(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(duplicate, DaemonError::InvalidRequest(_)));

        let created = create_issue(
            &pool,
            CreateIssueRequest {
                project_id: "project-1".to_owned(),
                title: "First".to_owned(),
                description: "First issue.".to_owned(),
                acceptance_criteria: Vec::new(),
                external_references: Vec::new(),
                dependencies: vec!["ISSUE-003".to_owned()],
                blocking_facts: Vec::new(),
                verification_level: VerificationLevel::AgentSelf,
                verification_steps: Vec::new(),
            },
        )
        .await
        .unwrap();
        assert_eq!(created.issue_key, "ISSUE-001");

        let self_dependency = update_issue(
            &pool,
            UpdateIssueRequest {
                project_id: "project-1".to_owned(),
                issue_key: "ISSUE-001".to_owned(),
                expected_revision: created.revision,
                title: None,
                description: None,
                acceptance_criteria: None,
                external_references: None,
                dependencies: Some(vec!["ISSUE-001".to_owned()]),
                blocking_facts: None,
                verification_level: None,
                verification_steps: None,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(self_dependency, DaemonError::InvalidRequest(_)));

        let cycle = update_issue(
            &pool,
            UpdateIssueRequest {
                project_id: "project-1".to_owned(),
                issue_key: "ISSUE-004".to_owned(),
                expected_revision: 1,
                title: None,
                description: None,
                acceptance_criteria: None,
                external_references: None,
                dependencies: Some(vec!["ISSUE-001".to_owned()]),
                blocking_facts: None,
                verification_level: None,
                verification_steps: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(cycle.board_state, IssueBoardState::Todo);

        let cycle = update_issue(
            &pool,
            UpdateIssueRequest {
                project_id: "project-1".to_owned(),
                issue_key: "ISSUE-001".to_owned(),
                expected_revision: created.revision,
                title: None,
                description: None,
                acceptance_criteria: None,
                external_references: None,
                dependencies: Some(vec!["ISSUE-003".to_owned(), "ISSUE-004".to_owned()]),
                blocking_facts: None,
                verification_level: None,
                verification_steps: None,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(cycle, DaemonError::InvalidRequest(_)));
        assert!(cycle.to_string().contains("cycle"));

        // The rejected cycle must not have been committed: ISSUE-001 still
        // depends only on ISSUE-003.
        let board = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T01:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        let dependent = board
            .iter()
            .find(|card| card.issue_key == "ISSUE-001")
            .unwrap();
        assert_eq!(dependent.dependencies.len(), 1);
        assert_eq!(dependent.dependencies[0].issue_key, "ISSUE-003");
    }

    #[tokio::test]
    async fn issue_blocking_facts_round_trip_and_satisfied_facts_stop_blocking() {
        let pool = run_pool().await;
        let created = create_issue(
            &pool,
            CreateIssueRequest {
                project_id: "project-1".to_owned(),
                title: "Zed integration".to_owned(),
                description: "Depends on a host capability.".to_owned(),
                acceptance_criteria: Vec::new(),
                external_references: Vec::new(),
                dependencies: Vec::new(),
                blocking_facts: vec![blocking_fact(
                    "host:zed-hooks",
                    IssueBlockingFactKind::HostCapability,
                    "Zed does not provide lifecycle hooks yet",
                    false,
                )],
                verification_level: VerificationLevel::AgentSelf,
                verification_steps: Vec::new(),
            },
        )
        .await
        .unwrap();

        let board = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T01:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        let card = &board[0];
        assert!(card.blocked);
        assert_eq!(card.blocking_facts.len(), 1);
        assert_eq!(card.blocking_reasons.len(), 1);
        assert_eq!(card.blocking_reasons[0].kind, IssueBlockingReasonKind::Fact);
        assert_eq!(
            card.blocking_reasons[0].fact_id.as_deref(),
            Some("host:zed-hooks")
        );

        let satisfied = update_issue(
            &pool,
            UpdateIssueRequest {
                project_id: "project-1".to_owned(),
                issue_key: created.issue_key.clone(),
                expected_revision: created.revision,
                title: None,
                description: None,
                acceptance_criteria: None,
                external_references: None,
                dependencies: None,
                blocking_facts: Some(vec![blocking_fact(
                    "host:zed-hooks",
                    IssueBlockingFactKind::HostCapability,
                    "Zed now provides lifecycle hooks",
                    true,
                )]),
                verification_level: None,
                verification_steps: None,
            },
        )
        .await
        .unwrap();
        let board = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T02:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        let card = &board[0];
        assert!(!card.blocked);
        assert!(card.blocking_reasons.is_empty());
        assert!(card.blocking_facts[0].satisfied);

        let cleared = update_issue(
            &pool,
            UpdateIssueRequest {
                project_id: "project-1".to_owned(),
                issue_key: created.issue_key,
                expected_revision: satisfied.revision,
                title: None,
                description: None,
                acceptance_criteria: None,
                external_references: None,
                dependencies: None,
                blocking_facts: Some(Vec::new()),
                verification_level: None,
                verification_steps: None,
            },
        )
        .await
        .unwrap();
        let board = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T03:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        assert!(board[0].blocking_facts.is_empty());
        assert_eq!(cleared.revision, 3);
    }

    #[tokio::test]
    async fn deleting_an_issue_removes_its_dependency_edges_in_both_directions() {
        let pool = run_pool().await;
        native_issue(&pool, "project-1", 3).await;
        let created = create_issue(
            &pool,
            CreateIssueRequest {
                project_id: "project-1".to_owned(),
                title: "Depends on 3".to_owned(),
                description: "Dependency on ISSUE-003.".to_owned(),
                acceptance_criteria: Vec::new(),
                external_references: Vec::new(),
                dependencies: vec!["ISSUE-003".to_owned()],
                blocking_facts: Vec::new(),
                verification_level: VerificationLevel::AgentSelf,
                verification_steps: Vec::new(),
            },
        )
        .await
        .unwrap();
        remove_issue(
            &pool,
            RemoveIssueRequest {
                project_id: "project-1".to_owned(),
                issue_number: 3,
                expected_revision: 1,
                action: IssueRemovalAction::Delete,
            },
        )
        .await
        .unwrap();
        let board = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T01:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        assert_eq!(board.len(), 1);
        assert_eq!(board[0].issue_key, created.issue_key);
        assert!(board[0].dependencies.is_empty());
        assert!(!board[0].blocked);
    }

    #[test]
    fn issue_blocking_fact_validation_rejects_unsafe_or_oversized_values() {
        let empty_fact_id = blocking_fact(" ", IssueBlockingFactKind::External, "Empty id", false);
        assert!(normalize_issue_blocking_facts(&[empty_fact_id]).is_err());
        let empty_description = blocking_fact(
            "host:zed-hooks",
            IssueBlockingFactKind::External,
            " ",
            false,
        );
        assert!(normalize_issue_blocking_facts(&[empty_description]).is_err());
        let oversized_id = blocking_fact(
            &"a".repeat(MAX_ISSUE_FACT_ID_BYTES + 1),
            IssueBlockingFactKind::External,
            "Oversized fact id",
            false,
        );
        assert!(normalize_issue_blocking_facts(&[oversized_id]).is_err());
        let oversized_description = blocking_fact(
            "host:zed-hooks",
            IssueBlockingFactKind::External,
            &"a".repeat(MAX_ISSUE_FACT_DESCRIPTION_BYTES + 1),
            false,
        );
        assert!(normalize_issue_blocking_facts(&[oversized_description]).is_err());
        let duplicate = vec![
            blocking_fact("host:a", IssueBlockingFactKind::External, "First", false),
            blocking_fact("host:a", IssueBlockingFactKind::External, "Second", true),
        ];
        assert!(normalize_issue_blocking_facts(&duplicate).is_err());
        let too_many = (0..=MAX_ISSUE_BLOCKING_FACTS)
            .map(|index| {
                blocking_fact(
                    &format!("fact:{index}"),
                    IssueBlockingFactKind::External,
                    "Too many",
                    false,
                )
            })
            .collect::<Vec<_>>();
        assert!(normalize_issue_blocking_facts(&too_many).is_err());
    }

    #[tokio::test]
    async fn done_issues_can_be_archived_but_not_deleted() {
        let pool = run_pool().await;
        native_issue(&pool, "project-1", 3).await;
        sqlx::query(
            "UPDATE native_issues SET status = 'done', revision = 2,
                    closed_at = '2026-08-06T10:00:00.000Z'
             WHERE project_id = 'project-1' AND issue_number = 3",
        )
        .execute(&pool)
        .await
        .unwrap();

        let delete_error = remove_issue(
            &pool,
            RemoveIssueRequest {
                project_id: "project-1".to_owned(),
                issue_number: 3,
                expected_revision: 2,
                action: IssueRemovalAction::Delete,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(
            delete_error,
            DaemonError::State {
                code: "agent_run_conflict",
                ..
            }
        ));

        let archived = remove_issue(
            &pool,
            RemoveIssueRequest {
                project_id: "project-1".to_owned(),
                issue_number: 3,
                expected_revision: 2,
                action: IssueRemovalAction::Archive,
            },
        )
        .await
        .unwrap();
        assert_eq!(archived.action, IssueRemovalAction::Archive);

        let cards = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T11:00:00.000Z",
            "2026-08-05T11:00:00.000Z",
        )
        .await
        .unwrap();
        assert!(cards.is_empty());
        assert_eq!(
            resolve_native_issue_identity(&pool, &archived.issue_id)
                .await
                .unwrap(),
            Some(("project-1".to_owned(), 3))
        );
        let detail = load_native_issue_detail(
            &pool,
            "project-1",
            3,
            &[],
            "2026-08-06T11:00:00.000Z",
            "2026-08-05T11:00:00.000Z",
        )
        .await
        .unwrap();
        assert!(detail.issue.archived_at.is_some());
        assert_eq!(detail.issue.state_revision, 3);
    }

    #[tokio::test]
    async fn non_done_issue_delete_unlinks_but_retains_agent_runs() {
        let pool = run_pool().await;
        native_issue(&pool, "project-1", 3).await;
        let started = record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_delete_started_issue",
                Some("root:delete-started-issue"),
                AgentRunEventType::Started,
            ),
        )
        .await
        .unwrap()
        .run
        .unwrap();
        let bound = start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(started.run_id.clone()),
                issue_key: "ISSUE-003".to_owned(),
                expected_revision: Some(started.revision),
                session_id: None,
            },
        )
        .await
        .unwrap();

        let deleted = remove_issue(
            &pool,
            RemoveIssueRequest {
                project_id: "project-1".to_owned(),
                issue_number: 3,
                expected_revision: bound.state_revision,
                action: IssueRemovalAction::Delete,
            },
        )
        .await
        .unwrap();
        assert_eq!(deleted.action, IssueRemovalAction::Delete);
        assert_eq!(
            resolve_native_issue_identity(&pool, &deleted.issue_id)
                .await
                .unwrap(),
            None
        );
        let retained_run = load_agent_run(&pool, &started.run_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained_run.issue_number, None);
    }

    #[test]
    fn legacy_issue_content_is_normalized_for_one_time_native_import() {
        let (description, criteria) = parse_legacy_issue_content(
            "# ISSUE-007 — Export Issues\n\n| Field | Value |\n| --- | --- |\n| Status | Open |\n\nKeep native data authoritative.\n\n## Acceptance Criteria\n\n- [ ] Export stable keys\n- [x] Preserve descriptions\n\n## Notes\n\nNo live document link.",
        );

        assert_eq!(
            description,
            "Keep native data authoritative.\n\n## Notes\n\nNo live document link."
        );
        assert_eq!(criteria, ["Export stable keys", "Preserve descriptions"]);
    }

    #[tokio::test]
    async fn in_review_requires_an_explicit_root_decision_even_after_stop() {
        let pool = run_pool().await;
        native_issue(&pool, "project-1", 3).await;
        let started = record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_closure_start",
                Some("root:closure-turn"),
                AgentRunEventType::Started,
            ),
        )
        .await
        .unwrap()
        .run
        .unwrap();
        let in_progress = start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(started.run_id.clone()),
                issue_key: "ISSUE-003".to_owned(),
                expected_revision: Some(started.revision),
                session_id: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(in_progress.board_state, IssueBoardState::InProgress);
        let started_issue = {
            let mut tx = pool.begin().await.unwrap();
            let issue = load_native_issue_tx(&mut tx, "project-1", 3)
                .await
                .unwrap()
                .unwrap();
            tx.commit().await.unwrap();
            issue
        };
        assert!(started_issue.started_at.is_some());
        assert_eq!(started_issue.closed_at, None);

        let stopped = record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_closure_stop",
                Some("root:closure-turn"),
                AgentRunEventType::Ended,
            ),
        )
        .await
        .unwrap()
        .run
        .unwrap();
        let state_after_stop = {
            let mut tx = pool.begin().await.unwrap();
            let state = load_native_issue_tx(&mut tx, "project-1", 3)
                .await
                .unwrap()
                .unwrap();
            tx.commit().await.unwrap();
            state
        };
        assert_eq!(state_after_stop.board_state, IssueBoardState::InProgress);
        assert_eq!(stopped.outcome, None);

        let request = RequestIssueClosureRequest {
            project_id: "project-1".to_owned(),
            run_id: Some(stopped.run_id),
            issue_key: None,
            summary: Some("Acceptance criteria are satisfied".to_owned()),
            expected_revision: Some(stopped.revision),
        };
        let closure = request_issue_closure(&pool, request.clone()).await.unwrap();
        assert_eq!(closure.board_state, IssueBoardState::InReview);
        let repeated = request_issue_closure(&pool, request).await.unwrap();
        assert_eq!(repeated.state_revision, closure.state_revision);

        let approved = apply_issue_gate(
            &pool,
            ApplyIssueGateRequest {
                project_id: "project-1".to_owned(),
                issue_number: 3,
                expected_revision: closure.state_revision,
                action: IssueGateAction::ApproveClosure,
            },
        )
        .await
        .unwrap();
        assert_eq!(approved.board_state, IssueBoardState::Done);
        let approved_issue = {
            let mut tx = pool.begin().await.unwrap();
            let issue = load_native_issue_tx(&mut tx, "project-1", 3)
                .await
                .unwrap()
                .unwrap();
            tx.commit().await.unwrap();
            issue
        };
        assert_eq!(
            approved_issue.closure_summary.as_deref(),
            Some("Acceptance criteria are satisfied")
        );
        assert_eq!(approved_issue.started_at, started_issue.started_at);
        assert!(approved_issue.closed_at.is_some());

        let reopened = apply_issue_gate(
            &pool,
            ApplyIssueGateRequest {
                project_id: "project-1".to_owned(),
                issue_number: 3,
                expected_revision: approved.revision,
                action: IssueGateAction::Reopen,
            },
        )
        .await
        .unwrap();
        assert_eq!(reopened.board_state, IssueBoardState::Todo);
        let reopened_issue = {
            let mut tx = pool.begin().await.unwrap();
            let issue = load_native_issue_tx(&mut tx, "project-1", 3)
                .await
                .unwrap()
                .unwrap();
            tx.commit().await.unwrap();
            issue
        };
        assert_eq!(reopened_issue.closure_summary, None);
        assert_eq!(reopened_issue.started_at, None);
        assert_eq!(reopened_issue.closed_at, None);
    }

    #[tokio::test]
    async fn issue_workflow_mutations_hide_runs_owned_by_another_project() {
        let pool = run_pool().await;
        native_issue(&pool, "project-2", 3).await;
        let started = record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_cross_project_start",
                Some("root:cross-project-turn"),
                AgentRunEventType::Started,
            ),
        )
        .await
        .unwrap()
        .run
        .unwrap();

        let start_error = start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-2".to_owned(),
                run_id: Some(started.run_id.clone()),
                issue_key: "ISSUE-003".to_owned(),
                expected_revision: Some(started.revision),
                session_id: None,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(start_error, DaemonError::NotFound(_)));

        let closure_error = request_issue_closure(
            &pool,
            RequestIssueClosureRequest {
                project_id: "project-2".to_owned(),
                run_id: Some(started.run_id.clone()),
                issue_key: None,
                summary: None,
                expected_revision: Some(started.revision),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(closure_error, DaemonError::NotFound(_)));

        let unchanged = load_agent_run(&pool, &started.run_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unchanged.project_id, "project-1");
        assert_eq!(unchanged.phase, AgentRunPhase::Running);
        assert_eq!(unchanged.issue_number, None);
    }

    #[test]
    fn issue_parser_reads_lifecycle_title_and_found_date() {
        let resources = [resource(
            "issues/open/003_issue_board.md",
            "# ISSUE-003：Agent Issue board\n\n| 字段 | 内容 |\n| --- | --- |\n| 类型 | feature |\n| 优先级 | P1 |\n| 影响组件 | `daemon`、`macOS` |\n| 发现日期 | 2026-08-06 |",
        )];
        let (issues, diagnostics) =
            project_issue_board("project-1", &resources, &[], "2026-08-06T12:00:00.000Z");
        assert!(diagnostics.is_empty());
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].issue_number, 3);
        assert_eq!(issues[0].issue_key, "ISSUE-003");
        assert_eq!(issues[0].title, "Agent Issue board");
        assert_eq!(issues[0].found_at.as_deref(), Some("2026-08-06"));
        assert_eq!(issues[0].board_state, IssueBoardState::Todo);
    }

    #[test]
    fn issue_keys_and_paths_follow_the_exact_local_issue_naming_rule() {
        assert_eq!(parse_issue_reference("ISSUE-003").unwrap(), 3);
        for issue_key in [
            "ISSUE-000",
            "ISSUE-03",
            "ISSUE-0003",
            "ISSUE-1000",
            "issue-003",
            "ISSUE-ABC",
        ] {
            assert!(
                parse_issue_reference(issue_key).is_err(),
                "{issue_key} must be rejected"
            );
        }
        assert!(parse_h1("# ISSUE-0003: Four digits").is_none());
        assert!(parse_h1("# ISSUE-000: Reserved number").is_none());
        assert!(parse_h1("# issue-003: Lowercase key").is_none());

        let resources = [
            resource(
                "issues/open/003_valid_issue_2.md",
                "# ISSUE-003: Valid Issue",
            ),
            resource("issues/open/000_reserved.md", "# ISSUE-000: Reserved"),
            resource("issues/open/0004_four_digits.md", "# ISSUE-004: Four"),
            resource("issues/open/004_Upper_case.md", "# ISSUE-004: Upper"),
            resource("issues/open/005_kebab-case.md", "# ISSUE-005: Kebab"),
            resource(
                "issues/open/006_double__underscore.md",
                "# ISSUE-006: Double",
            ),
            resource("issues/open/007_trailing_.md", "# ISSUE-007: Trailing"),
            resource("issues/open/008__leading.md", "# ISSUE-008: Leading"),
        ];
        let (issues, diagnostics) =
            project_issue_board("project-1", &resources, &[], "2026-08-06T12:00:00.000Z");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].issue_key, "ISSUE-003");
        assert_eq!(
            diagnostics
                .iter()
                .filter(|item| item.code == IssueBoardDiagnosticCode::MalformedPath)
                .count(),
            resources.len() - 1
        );
    }

    #[test]
    fn malformed_title_is_preserved_but_duplicate_number_is_quarantined() {
        let resources = [
            resource("issues/open/003_first.md", "No H1 here"),
            resource("issues/closed/003_second.md", "# ISSUE-004: Wrong number"),
            resource("issues/open/not-an-issue.md", "# ISSUE-005: Bad path"),
        ];
        let (issues, diagnostics) =
            project_issue_board("project-1", &resources, &[], "2026-08-06T12:00:00.000Z");
        assert!(issues.is_empty());
        assert!(
            diagnostics
                .iter()
                .any(|item| { item.code == IssueBoardDiagnosticCode::MalformedTitle })
        );
        assert!(
            diagnostics
                .iter()
                .any(|item| { item.code == IssueBoardDiagnosticCode::TitleNumberMismatch })
        );
        assert_eq!(
            diagnostics
                .iter()
                .filter(|item| item.code == IssueBoardDiagnosticCode::DuplicateIssueNumber)
                .count(),
            2
        );
        assert!(
            diagnostics
                .iter()
                .any(|item| { item.code == IssueBoardDiagnosticCode::MalformedPath })
        );
    }

    #[tokio::test]
    async fn reconciliation_keeps_stale_as_a_facet_and_reopen_resets_to_todo() {
        let pool = run_pool().await;
        let mut tx = pool.begin().await.unwrap();
        set_issue_workflow_state_tx(
            &mut tx,
            "project-1",
            3,
            IssueBoardState::InProgress,
            None,
            None,
            "2026-08-04T09:00:00.000Z",
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let ended = run(
            "arun_stale",
            AgentRunPhase::Ended,
            None,
            "2026-08-04T10:00:00.000Z",
            "2026-08-04T10:00:00.000Z",
        );
        let (mut open_cards, _) = project_issue_board(
            "project-1",
            &[resource(
                "issues/open/003_issue_board.md",
                "# ISSUE-003: Issue board",
            )],
            &[ended],
            "2026-08-06T12:00:00.000Z",
        );
        reconcile_issue_workflow_states(
            &pool,
            "project-1",
            &mut open_cards,
            "2026-08-05T12:00:00.000Z",
        )
        .await
        .unwrap();
        assert_eq!(open_cards[0].board_state, IssueBoardState::InProgress);
        assert!(open_cards[0].is_stale);

        let (mut closed_cards, _) = project_issue_board(
            "project-1",
            &[resource(
                "issues/closed/003_issue_board.md",
                "# ISSUE-003: Issue board",
            )],
            &[],
            "2026-08-06T12:00:00.000Z",
        );
        reconcile_issue_workflow_states(
            &pool,
            "project-1",
            &mut closed_cards,
            "2026-08-05T12:00:00.000Z",
        )
        .await
        .unwrap();
        assert_eq!(closed_cards[0].board_state, IssueBoardState::Done);
        assert!(!closed_cards[0].is_stale);

        let (mut reopened_cards, _) = project_issue_board(
            "project-1",
            &[resource(
                "issues/open/003_issue_board.md",
                "# ISSUE-003: Issue board",
            )],
            &[],
            "2026-08-06T12:00:00.000Z",
        );
        reconcile_issue_workflow_states(
            &pool,
            "project-1",
            &mut reopened_cards,
            "2026-08-05T12:00:00.000Z",
        )
        .await
        .unwrap();
        assert_eq!(reopened_cards[0].board_state, IssueBoardState::Todo);
        assert!(!reopened_cards[0].is_stale);
    }

    #[test]
    fn run_projection_tracks_activity_without_deciding_board_state() {
        let now = "2026-08-06T12:00:00.000Z";
        let active = run(
            "arun_active",
            AgentRunPhase::Running,
            None,
            "2026-08-06T11:00:00.000Z",
            "2026-08-06T13:00:00.000Z",
        );
        let active_projection = project_runs(&[active], now);
        assert_eq!(active_projection.active_runs.len(), 1);
        assert_eq!(
            active_projection.latest_run.as_ref().unwrap().run_id,
            "arun_active"
        );

        let expired = run(
            "arun_expired",
            AgentRunPhase::Running,
            None,
            "2026-08-06T09:00:00.000Z",
            "2026-08-06T11:00:00.000Z",
        );
        let projection = project_runs(&[expired], now);
        assert!(projection.active_runs.is_empty());
        assert_eq!(
            projection.latest_run.unwrap().end_reason.as_deref(),
            Some(LEASE_EXPIRED_REASON)
        );
    }

    #[test]
    fn latest_run_order_uses_meaningful_timestamp_then_run_id() {
        let now = "2026-08-06T12:00:00.000Z";
        let lower = run(
            "arun_a",
            AgentRunPhase::Ended,
            Some(AgentRunOutcome::Failed),
            "2026-08-06T10:00:00.000Z",
            "2026-08-06T10:00:00.000Z",
        );
        let higher = run(
            "arun_b",
            AgentRunPhase::Ended,
            Some(AgentRunOutcome::Completed),
            "2026-08-06T10:00:00.000Z",
            "2026-08-06T10:00:00.000Z",
        );
        let projection = project_runs(&[lower, higher], now);
        assert_eq!(projection.latest_run.unwrap().run_id, "arun_b");
    }

    #[tokio::test]
    async fn begin_work_without_run_is_rejected() {
        let pool = run_pool().await;
        native_issue(&pool, "project-1", 3).await;
        let error = start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: None,
                issue_key: "ISSUE-003".to_owned(),
                expected_revision: None,
                session_id: None,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(error, DaemonError::InvalidRequest(_)));
        let board = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T01:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        assert_eq!(board[0].board_state, IssueBoardState::Todo);
    }

    #[tokio::test]
    async fn begin_work_rejects_an_issue_claimed_by_another_active_run() {
        let pool = run_pool().await;
        native_issue(&pool, "project-1", 3).await;
        native_issue(&pool, "project-1", 4).await;
        let started = record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_claim_start",
                Some("root:claim"),
                AgentRunEventType::Started,
            ),
        )
        .await
        .unwrap()
        .run
        .unwrap();
        start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(started.run_id.clone()),
                issue_key: "ISSUE-003".to_owned(),
                expected_revision: Some(started.revision),
                session_id: None,
            },
        )
        .await
        .unwrap();

        // The same run cannot claim a second Issue.
        let error = start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(started.run_id.clone()),
                issue_key: "ISSUE-004".to_owned(),
                expected_revision: Some(started.revision),
                session_id: None,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            DaemonError::State {
                code: "agent_run_conflict",
                ..
            }
        ));

        // A hook run from another session cannot claim the same Issue while
        // the first run remains active.
        let second =
            session_hook_start(&pool, "hook_claim_start_2", "root:claim-2", "session-2").await;
        let error = start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(second.run_id),
                issue_key: "ISSUE-003".to_owned(),
                expected_revision: Some(second.revision),
                session_id: None,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            DaemonError::State {
                code: "agent_run_conflict",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn closure_without_run_is_rejected() {
        let pool = run_pool().await;
        native_issue(&pool, "project-1", 3).await;
        let error = request_issue_closure(
            &pool,
            RequestIssueClosureRequest {
                project_id: "project-1".to_owned(),
                run_id: None,
                issue_key: Some("ISSUE-003".to_owned()),
                summary: Some("Acceptance criteria are satisfied".to_owned()),
                expected_revision: None,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(error, DaemonError::InvalidRequest(_)));
        let board = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T01:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        assert_eq!(board[0].board_state, IssueBoardState::Todo);
    }

    #[tokio::test]
    async fn closure_requires_the_run_that_started_work() {
        let pool = run_pool().await;
        native_issue(&pool, "project-1", 3).await;
        let started = record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_closure_start",
                Some("root:closure"),
                AgentRunEventType::Started,
            ),
        )
        .await
        .unwrap()
        .run
        .unwrap();
        start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(started.run_id.clone()),
                issue_key: "ISSUE-003".to_owned(),
                expected_revision: Some(started.revision),
                session_id: None,
            },
        )
        .await
        .unwrap();

        // A different root run that never started work on the Issue cannot
        // request closure.
        let other = record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_closure_other",
                Some("root:other"),
                AgentRunEventType::Started,
            ),
        )
        .await
        .unwrap()
        .run
        .unwrap();
        let error = request_issue_closure(
            &pool,
            RequestIssueClosureRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(other.run_id),
                issue_key: None,
                summary: Some("Acceptance criteria are satisfied".to_owned()),
                expected_revision: Some(other.revision),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(error, DaemonError::InvalidRequest(_)));

        // start_issue_work bumped the run revision; reload it for closure.
        let started_after_start = load_agent_run(&pool, &started.run_id)
            .await
            .unwrap()
            .unwrap();
        let closure = request_issue_closure(
            &pool,
            RequestIssueClosureRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(started.run_id),
                issue_key: None,
                summary: Some("Acceptance criteria are satisfied".to_owned()),
                expected_revision: Some(started_after_start.revision),
            },
        )
        .await
        .unwrap();
        assert_eq!(closure.board_state, IssueBoardState::InReview);
        assert_eq!(closure.run.as_ref().unwrap().host, AgentRunHost::Codex);
        assert_eq!(closure.run.as_ref().unwrap().issue_number, Some(3));
    }

    #[tokio::test]
    async fn unclaim_releases_in_progress_back_to_todo() {
        let pool = run_pool().await;
        native_issue(&pool, "project-1", 3).await;
        let run = record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_unclaim_start",
                Some("root:unclaim"),
                AgentRunEventType::Started,
            ),
        )
        .await
        .unwrap()
        .run
        .unwrap();
        let started = start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(run.run_id.clone()),
                issue_key: "ISSUE-003".to_owned(),
                expected_revision: Some(run.revision),
                session_id: None,
            },
        )
        .await
        .unwrap();

        let released = unclaim_issue(
            &pool,
            UnclaimIssueRequest {
                project_id: "project-1".to_owned(),
                issue_key: "ISSUE-003".to_owned(),
                expected_revision: started.state_revision,
                run_id: Some(run.run_id.clone()),
            },
        )
        .await
        .unwrap();
        assert_eq!(released.board_state, IssueBoardState::Todo);
        let unbound = load_agent_run(&pool, &run.run_id).await.unwrap().unwrap();
        assert_eq!(unbound.issue_number, None);
        let board = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T02:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        assert_eq!(board[0].board_state, IssueBoardState::Todo);
        assert!(board[0].changed_by_run_id.is_none());
    }

    #[tokio::test]
    async fn unclaim_rejects_a_run_not_bound_to_the_issue() {
        let pool = run_pool().await;
        native_issue(&pool, "project-1", 3).await;
        native_issue(&pool, "project-1", 4).await;
        let run = record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_unclaim_other",
                Some("root:other"),
                AgentRunEventType::Started,
            ),
        )
        .await
        .unwrap()
        .run
        .unwrap();
        let error = unclaim_issue(
            &pool,
            UnclaimIssueRequest {
                project_id: "project-1".to_owned(),
                issue_key: "ISSUE-003".to_owned(),
                expected_revision: 1,
                run_id: Some(run.run_id),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            DaemonError::State {
                code: "agent_run_conflict",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn unclaim_without_run_releases_paused_issue_back_to_todo() {
        let pool = run_pool().await;
        native_issue(&pool, "project-1", 3).await;
        let run = record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_manual_unclaim_start",
                Some("root:manual-unclaim"),
                AgentRunEventType::Started,
            ),
        )
        .await
        .unwrap()
        .run
        .unwrap();
        let _started = start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(run.run_id.clone()),
                issue_key: "ISSUE-003".to_owned(),
                expected_revision: Some(run.revision),
                session_id: None,
            },
        )
        .await
        .unwrap();
        let paused = pause_issue_work(
            &pool,
            PauseIssueRequest {
                project_id: "project-1".to_owned(),
                run_id: run.run_id.clone(),
                issue_key: "ISSUE-003".to_owned(),
            },
        )
        .await
        .unwrap();
        assert_eq!(paused.board_state, IssueBoardState::Paused);

        // The pausing session ends; the run stays bound but is no longer active.
        record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_manual_unclaim_stop",
                Some("root:manual-unclaim"),
                AgentRunEventType::Ended,
            ),
        )
        .await
        .unwrap();

        let released = unclaim_issue(
            &pool,
            UnclaimIssueRequest {
                project_id: "project-1".to_owned(),
                issue_key: "ISSUE-003".to_owned(),
                expected_revision: paused.state_revision,
                run_id: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(released.board_state, IssueBoardState::Todo);
        let unbound = load_agent_run(&pool, &run.run_id).await.unwrap().unwrap();
        assert_eq!(unbound.issue_number, None);
        let board = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T02:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        assert_eq!(board[0].board_state, IssueBoardState::Todo);
        assert!(board[0].changed_by_run_id.is_none());
    }

    #[tokio::test]
    async fn unclaim_without_run_rejects_while_an_active_run_works_the_issue() {
        let pool = run_pool().await;
        native_issue(&pool, "project-1", 3).await;
        let run = record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_manual_unclaim_active",
                Some("root:manual-unclaim-active"),
                AgentRunEventType::Started,
            ),
        )
        .await
        .unwrap()
        .run
        .unwrap();
        let started = start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(run.run_id.clone()),
                issue_key: "ISSUE-003".to_owned(),
                expected_revision: Some(run.revision),
                session_id: None,
            },
        )
        .await
        .unwrap();

        let error = unclaim_issue(
            &pool,
            UnclaimIssueRequest {
                project_id: "project-1".to_owned(),
                issue_key: "ISSUE-003".to_owned(),
                expected_revision: started.state_revision,
                run_id: None,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            DaemonError::State {
                code: "agent_run_conflict",
                ..
            }
        ));
        let still_bound = load_agent_run(&pool, &run.run_id).await.unwrap().unwrap();
        assert_eq!(still_bound.issue_number, Some(3));
    }

    #[tokio::test]
    async fn resume_without_run_resumes_paused_issue() {
        let pool = run_pool().await;
        native_issue(&pool, "project-1", 3).await;
        let run = record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_manual_resume_start",
                Some("root:manual-resume"),
                AgentRunEventType::Started,
            ),
        )
        .await
        .unwrap()
        .run
        .unwrap();
        let _started = start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(run.run_id.clone()),
                issue_key: "ISSUE-003".to_owned(),
                expected_revision: Some(run.revision),
                session_id: None,
            },
        )
        .await
        .unwrap();
        pause_issue_work(
            &pool,
            PauseIssueRequest {
                project_id: "project-1".to_owned(),
                run_id: run.run_id.clone(),
                issue_key: "ISSUE-003".to_owned(),
            },
        )
        .await
        .unwrap();
        record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_manual_resume_stop",
                Some("root:manual-resume"),
                AgentRunEventType::Ended,
            ),
        )
        .await
        .unwrap();

        let resumed = resume_issue_work(
            &pool,
            ResumeIssueRequest {
                project_id: "project-1".to_owned(),
                run_id: None,
                issue_key: "ISSUE-003".to_owned(),
                takeover: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(resumed.board_state, IssueBoardState::InProgress);
        assert!(resumed.run.is_none());
        let unbound = load_agent_run(&pool, &run.run_id).await.unwrap().unwrap();
        assert_eq!(unbound.issue_number, None);
        let board = project_native_issue_board(
            &pool,
            "project-1",
            &[],
            "2026-08-06T02:00:00.000Z",
            "2026-08-05T01:00:00.000Z",
        )
        .await
        .unwrap();
        assert_eq!(board[0].board_state, IssueBoardState::InProgress);
        assert!(board[0].changed_by_run_id.is_none());
    }

    #[tokio::test]
    async fn resume_without_run_rejects_while_the_pausing_run_is_still_active() {
        let pool = run_pool().await;
        native_issue(&pool, "project-1", 3).await;
        let run = record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_manual_resume_active",
                Some("root:manual-resume-active"),
                AgentRunEventType::Started,
            ),
        )
        .await
        .unwrap()
        .run
        .unwrap();
        start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(run.run_id.clone()),
                issue_key: "ISSUE-003".to_owned(),
                expected_revision: Some(run.revision),
                session_id: None,
            },
        )
        .await
        .unwrap();
        // Pause without ending the run: the session still holds the Issue.
        pause_issue_work(
            &pool,
            PauseIssueRequest {
                project_id: "project-1".to_owned(),
                run_id: run.run_id.clone(),
                issue_key: "ISSUE-003".to_owned(),
            },
        )
        .await
        .unwrap();

        let error = resume_issue_work(
            &pool,
            ResumeIssueRequest {
                project_id: "project-1".to_owned(),
                run_id: None,
                issue_key: "ISSUE-003".to_owned(),
                takeover: false,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            DaemonError::State {
                code: "agent_run_conflict",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn closure_requires_verification_steps_for_human_review() {
        let pool = run_pool().await;
        create_issue(
            &pool,
            CreateIssueRequest {
                project_id: "project-1".to_owned(),
                title: "Human reviewed".to_owned(),
                description: "needs human".to_owned(),
                acceptance_criteria: vec![],
                external_references: Vec::new(),
                dependencies: Vec::new(),
                blocking_facts: Vec::new(),
                verification_level: VerificationLevel::HumanRequired,
                verification_steps: Vec::new(),
            },
        )
        .await
        .unwrap();
        let run = record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_verification_start",
                Some("root:verification"),
                AgentRunEventType::Started,
            ),
        )
        .await
        .unwrap()
        .run
        .unwrap();
        let started = start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(run.run_id.clone()),
                issue_key: "ISSUE-001".to_owned(),
                expected_revision: Some(run.revision),
                session_id: None,
            },
        )
        .await
        .unwrap();
        let run_after_start = load_agent_run(&pool, &run.run_id).await.unwrap().unwrap();
        let error = request_issue_closure(
            &pool,
            RequestIssueClosureRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(run.run_id.clone()),
                issue_key: None,
                summary: Some("done".to_owned()),
                expected_revision: Some(run_after_start.revision),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(error, DaemonError::InvalidRequest(_)));
        assert_eq!(started.board_state, IssueBoardState::InProgress);

        // Adding the steps unblocks the closure request.
        update_issue(
            &pool,
            UpdateIssueRequest {
                project_id: "project-1".to_owned(),
                issue_key: "ISSUE-001".to_owned(),
                expected_revision: started.state_revision,
                title: None,
                description: None,
                acceptance_criteria: None,
                external_references: None,
                dependencies: None,
                blocking_facts: None,
                verification_level: None,
                verification_steps: Some(vec![VerificationStep {
                    text: "Check the panel".to_owned(),
                    completed: false,
                }]),
            },
        )
        .await
        .unwrap();
        let run_final = load_agent_run(&pool, &run.run_id).await.unwrap().unwrap();
        let closure = request_issue_closure(
            &pool,
            RequestIssueClosureRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(run.run_id),
                issue_key: None,
                summary: Some("done".to_owned()),
                expected_revision: Some(run_final.revision),
            },
        )
        .await
        .unwrap();
        assert_eq!(closure.board_state, IssueBoardState::InReview);
    }

    #[tokio::test]
    async fn recover_stale_runs_ends_lease_expired_and_done_bound_runs() {
        let pool = run_pool().await;
        native_issue(&pool, "project-1", 3).await;
        // One run with an already-expired lease.
        let expired = record_agent_run_event(
            &pool,
            lifecycle_request(
                "hook_reaper_expired",
                Some("root:expired"),
                AgentRunEventType::Started,
            ),
        )
        .await
        .unwrap()
        .run
        .unwrap();
        sqlx::query(
            "UPDATE agent_runs SET lease_expires_at = '2020-01-01T00:00:00.000Z'
             WHERE run_id = $1",
        )
        .bind(&expired.run_id)
        .execute(&pool)
        .await
        .unwrap();
        // Another run whose Issue is closed while it is still running.
        let closed = session_hook_start(
            &pool,
            "hook_reaper_closed",
            "root:closed",
            "reaper-closed-session",
        )
        .await;
        start_issue_work(
            &pool,
            StartIssueWorkRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(closed.run_id.clone()),
                issue_key: "ISSUE-003".to_owned(),
                expected_revision: Some(closed.revision),
                session_id: None,
            },
        )
        .await
        .unwrap();
        // start_issue_work bumped the run revision; reload it for closure.
        let closed_after_start = load_agent_run(&pool, &closed.run_id)
            .await
            .unwrap()
            .unwrap();
        request_issue_closure(
            &pool,
            RequestIssueClosureRequest {
                project_id: "project-1".to_owned(),
                run_id: Some(closed.run_id.clone()),
                issue_key: None,
                summary: Some("done".to_owned()),
                expected_revision: Some(closed_after_start.revision),
            },
        )
        .await
        .unwrap();
        apply_issue_gate(
            &pool,
            ApplyIssueGateRequest {
                project_id: "project-1".to_owned(),
                issue_number: 3,
                expected_revision: 3,
                action: IssueGateAction::ApproveClosure,
            },
        )
        .await
        .unwrap();

        assert_eq!(recover_stale_runs(&pool).await.unwrap(), 2);
        let expired_row = load_agent_run(&pool, &expired.run_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(expired_row.phase, AgentRunPhase::Ended);
        assert_eq!(
            expired_row.end_reason.as_deref(),
            Some(LEASE_EXPIRED_REASON)
        );
        let closed_row = load_agent_run(&pool, &closed.run_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(closed_row.phase, AgentRunPhase::Ended);
        assert_eq!(closed_row.end_reason.as_deref(), Some(ISSUE_CLOSED_REASON));
        // A second sweep is a no-op.
        assert_eq!(recover_stale_runs(&pool).await.unwrap(), 0);
    }
}
