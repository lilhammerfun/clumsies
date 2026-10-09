//! Everything the client reads from and writes to the local engine.
//!
//! The daemon owns the session and the Project's local state; the client asks
//! it and never reaches the Server itself. Every function here returns a
//! message rather than panicking, because a signed-out or unreachable engine is
//! a state the screens draw.

mod diagnostics;
pub use diagnostics::*;
mod connections;
pub use connections::*;
mod codex_host;
mod memory;
pub use memory::*;

use std::collections::BTreeMap;
use std::thread::sleep;
use std::time::{Duration, Instant};

use clumsiesd::{
    DaemonContentDraftUpdate, DaemonCreateDraftOperation, DaemonDeleteDraftOperation,
    DaemonDiscardDraftOperation, DaemonDraftContent, DaemonDraftDetail, DaemonDraftListQuery,
    DaemonDraftOperation, DaemonDraftOperationRequest, DaemonDraftOperationResponse,
    DaemonDraftOperationSource, DaemonDraftResourceKind, DaemonDraftScope, DaemonDraftSummary,
    DaemonHealth, DaemonIpcClient, DaemonIpcRequest, DaemonLocalDraftStatus,
    DaemonProjectCacheClearRequest, DaemonProjectCheckout, DaemonProjectCheckoutRequest,
    DaemonProjectCheckoutResource, DaemonProjectSelectionRequest, DaemonProjectStorageAvailability,
    DaemonProjectStorageRequest, DaemonProjectStorageResetRequest, DaemonProjectSyncRetryRequest,
    DaemonRenameDraftOperation, DaemonRetryResponse, DaemonServerRequest, DaemonServerResponse,
    DaemonUpdateDraftOperation, DraftOperationSyncStatus, ErrorEnvelope, SyncRetryChannel,
};
use serde::{Deserialize, Serialize};

/// The service name the daemon registers. The client resolves it to the local
/// endpoint by the daemon's own rule, so both halves agree on where to talk.
const DAEMON_SERVICE: &str = "ai.clumsies.daemon";

/// How long a queued operation may take to reach the Server before the client
/// stops waiting. macOS gives the same barrier fifteen seconds.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(15);

/// How often the upload barrier asks the daemon again, which is macOS's 150ms.
const UPLOAD_POLL: Duration = Duration::from_millis(150);

/// Whether the local engine is reachable, and what it reports when it is.
pub enum EngineStatus {
    Connected(DaemonHealth),
    Unreachable(String),
}

/// One Project as the Server describes it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Project {
    pub project_id: String,
    pub name: String,
    /// What the space says about itself, which is the second thing a reader
    /// sets when the name alone is not enough.
    #[serde(default)]
    pub description: String,
    /// The revision the Server guards a write with: a change carries the one it
    /// read, and the answer carries the next.
    #[serde(default)]
    pub revision: i64,
}

#[derive(Deserialize)]
struct ProjectPage {
    items: Vec<Project>,
}

/// One Memory document in the Project's current Effective Memory. The resource
/// identity is what a draft operation updates, so it travels with the text.
#[derive(Clone)]
pub struct MemoryDocument {
    /// The Memory resource, as the daemon and the Server name it. A document
    /// that only exists as a proposal has no resource yet, so this is the draft
    /// that proposes it.
    pub resource_id: String,
    /// Whether the Project already holds this document. A proposal is written
    /// with the create operation, and throwing it away is what deleting it
    /// means.
    pub published: bool,
    pub is_directory: bool,
    pub org_owned: bool,
    pub path: String,
    /// What the Project publishes today.
    pub content: String,
    /// What an open draft proposes instead. The editor opens this, because it
    /// is what the reader last wrote, and the diff measures against
    /// [Self::content], because that is what a reviewer would see.
    pub draft_content: Option<String>,
    pub draft_deleted: bool,
}

/// A Project's checkout: its documents and the Project ref they resolved from.
pub struct Checkout {
    pub project_id: String,
    /// The commit the documents came from. A new draft is based on it.
    pub commit_id: Option<String>,
    pub documents: Vec<MemoryDocument>,
}

/// Where a Review stands, which is what decides whether it can still be
/// decided or published.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    /// Waiting for a decision.
    Open,
    /// Accepted, and waiting to be published.
    Approved,
    /// Sent back to its author, who may revise and resubmit it.
    Rejected,
    /// Published. The Review is complete.
    Merged,
}

impl ReviewStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Approved => "Approved",
            Self::Rejected => "Rejected",
            Self::Merged => "Merged",
        }
    }
}

/// Someone the Server names: the author of a proposal or a decision. Their
/// identity is not read yet, which is what telling the reader's own Reviews
/// apart will need.
#[derive(Clone, Debug, Deserialize)]
pub struct UserRef {
    pub email: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

impl UserRef {
    /// What to call this person: their name when the identity provider has one,
    /// and their address when it does not.
    pub fn name(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.email)
    }
}

/// The Review the Server created for a draft, and what a reader deciding it
/// needs to know.
#[derive(Clone, Debug, Deserialize)]
pub struct Review {
    pub review_id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub author: UserRef,
    pub status: ReviewStatus,
    /// Which Memory this would publish to. macOS defaults a missing scope to the
    /// Organization's, and so does this.
    #[serde(default = "Scope::org")]
    pub scope: Scope,
    /// The revision a decision or a merge has to name, so that two reviewers
    /// deciding at once cannot both win.
    pub version: i64,
    /// What the last decision said.
    #[serde(default)]
    pub decision_body: Option<String>,
    #[serde(default)]
    pub decided_by: Option<UserRef>,
    /// When the last decision was recorded, as the Server writes it.
    #[serde(default)]
    pub decided_at: Option<String>,
    pub updated_at: String,
    /// How the proposal stands against the reference it would publish to.
    pub coordination: Coordination,
}

impl Review {
    /// Whether a reader may approve or reject this Review now.
    pub fn can_decide(&self) -> bool {
        self.status == ReviewStatus::Open
    }

    /// Whether approving it would publish it. Approval is the merge transaction
    /// on the Server, so it waits on the same thing merging does: a base that is
    /// still the reference's head.
    pub fn can_approve(&self) -> bool {
        self.status == ReviewStatus::Open && self.coordination.is_current()
    }

    /// Whether an already approved Review may be published. A proposal whose
    /// base has moved on has to be reconciled first.
    pub fn can_merge(&self) -> bool {
        self.status == ReviewStatus::Approved && self.coordination.is_current()
    }
}

/// Which Memory a proposal publishes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Org,
    Project,
}

impl Scope {
    fn org() -> Self {
        Self::Org
    }
}

/// A proposal's relationship to the reference it would publish to.
#[derive(Clone, Debug, Deserialize)]
pub struct Coordination {
    /// The reference's head when the Server answered. A merge names it, so the
    /// publication cannot land on a head nobody reviewed.
    #[serde(default)]
    pub current_commit_id: Option<String>,
    #[serde(default)]
    pub freshness: Freshness,
    /// Whether upstream changed this resource since the proposal's base.
    #[serde(default)]
    pub has_upstream_resource_changes: bool,
    /// Whether reconciling with the current reference would need a choice from
    /// the author.
    #[serde(default)]
    pub reconciliation: Reconciliation,
}

impl Coordination {
    /// Whether the proposal's base is still the reference's head.
    pub fn is_current(&self) -> bool {
        self.freshness == Freshness::Current && !self.has_upstream_resource_changes
    }

    /// Whether the reference moved on in a way that conflicts with this
    /// proposal, which is the one state that stops a decision being honest.
    pub fn has_conflicts(&self) -> bool {
        self.freshness == Freshness::Behind && self.reconciliation == Reconciliation::Conflicts
    }
}

/// Whether the reference can be merged into a proposal without a choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reconciliation {
    Clean,
    Conflicts,
    /// Nothing has been computed yet, which is not a conflict. A state this
    /// client does not know lands here too, and is treated as no conflict
    /// rather than as one.
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Current,
    Behind,
    /// The Server names a state this client does not know, which it treats as
    /// "not current": publishing is the one thing that must not guess.
    #[default]
    #[serde(other)]
    Unknown,
}

/// What a Review proposes for one document.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewAction {
    Create,
    Update,
    Rename,
    Delete,
}

/// One document a Review would publish, with the text it proposes.
#[derive(Clone, Debug)]
pub struct ReviewedDocument {
    /// The resource the proposal targets. An update names this and not the
    /// path, so this is what ties a proposal to the document it changes.
    pub resource_id: Option<String>,
    /// The path the document would have, when the proposal names one.
    pub path: Option<String>,
    /// The text the proposal would publish. A rename or a delete carries none.
    pub content: Option<String>,
    pub action: ReviewAction,
}

impl ReviewedDocument {
    pub fn action_label(&self) -> &'static str {
        match self.action {
            ReviewAction::Create => "Added",
            ReviewAction::Update => "Changed",
            ReviewAction::Rename => "Renamed",
            ReviewAction::Delete => "Deleted",
        }
    }
}

/// A Review with everything a reader needs to decide it.
#[derive(Clone, Debug)]
pub struct ReviewDetail {
    pub review: Review,
    pub documents: Vec<ReviewedDocument>,
}

/// What the Server sends for a Review: the review, its proposals in either of
/// the two spellings it uses, and the discussion.
#[derive(Deserialize)]
struct ReviewDetailResponse {
    review: Review,
    /// The single-proposal spelling.
    #[serde(default)]
    draft: Option<DraftResponse>,
    #[serde(default)]
    operations: Vec<OperationResponse>,
    /// The batch spelling, which repeats the pair per proposal.
    #[serde(default)]
    drafts: Vec<DraftDetailResponse>,
}

#[derive(Deserialize)]
struct DraftDetailResponse {
    draft: DraftResponse,
    #[serde(default)]
    operations: Vec<OperationResponse>,
}

#[derive(Deserialize)]
struct DraftResponse {
    #[serde(default)]
    resource: Option<ResourceRefResponse>,
}

#[derive(Deserialize)]
struct ResourceRefResponse {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

/// One mutation in a proposal. Only what the diff needs is modelled: what the
/// operation does, to which resource, and the text it would publish.
#[derive(Deserialize)]
struct OperationResponse {
    action: ReviewAction,
    #[serde(default)]
    resource: Option<ResourceRefResponse>,
    #[serde(default)]
    content: Option<ContentResponse>,
    #[serde(default)]
    new_path: Option<String>,
}

#[derive(Deserialize)]
struct ContentResponse {
    content: String,
}

/// The documents a Review proposes, in the order the Server listed them.
fn reviewed_documents(
    draft: Option<DraftResponse>,
    operations: Vec<OperationResponse>,
    drafts: Vec<DraftDetailResponse>,
) -> Vec<ReviewedDocument> {
    let mut pairs: Vec<(Option<DraftResponse>, Vec<OperationResponse>)> = Vec::new();
    if !drafts.is_empty() {
        pairs.extend(
            drafts
                .into_iter()
                .map(|entry| (Some(entry.draft), entry.operations)),
        );
    } else if !operations.is_empty() {
        pairs.push((draft, operations));
    }
    pairs
        .into_iter()
        .flat_map(|(draft, operations)| {
            let fallback_path = draft
                .as_ref()
                .and_then(|draft| draft.resource.as_ref())
                .and_then(|resource| resource.path.clone());
            let fallback_id = draft
                .and_then(|draft| draft.resource)
                .and_then(|resource| resource.id);
            operations.into_iter().map(move |operation| {
                let resource = operation.resource.unwrap_or(ResourceRefResponse {
                    id: None,
                    path: None,
                });
                ReviewedDocument {
                    resource_id: resource.id.or_else(|| fallback_id.clone()),
                    path: operation
                        .new_path
                        .or(resource.path)
                        .or_else(|| fallback_path.clone()),
                    content: operation.content.map(|content| content.content),
                    action: operation.action,
                }
            })
        })
        .collect()
}

pub fn engine_status() -> EngineStatus {
    match client().health() {
        Ok(health) => EngineStatus::Connected(health),
        Err(error) => EngineStatus::Unreachable(error.to_string()),
    }
}

/// Where a Project's Memory lives on this machine, which is what the Project
/// settings dialog reads: macOS puts the same read-outs in its Memory Cache
/// section.
pub struct ProjectStorage {
    /// Where this Project's Memory is held.
    pub location: String,
    /// Which revision of that location this is, which the commands that change
    /// it have to name.
    pub location_revision: i64,
    /// How much of it is there.
    pub used_bytes: u64,
    pub status: &'static str,
    /// Why it is not ready, when it is not.
    pub diagnostic: Option<String>,
}

pub fn project_storage(project_id: &str) -> Result<ProjectStorage, String> {
    let storage = client()
        .project_storage(DaemonProjectStorageRequest {
            project_id: project_id.to_owned(),
        })
        .map_err(|error| error.to_string())?;
    Ok(ProjectStorage {
        location: storage.selected_root_path,
        location_revision: storage.location_revision,
        used_bytes: storage.size_bytes,
        status: match storage.availability {
            DaemonProjectStorageAvailability::Ready => "Ready",
            DaemonProjectStorageAvailability::Moving => "Moving",
            _ => "Unavailable",
        },
        diagnostic: storage.diagnostic,
    })
}

/// The Projects this account can reach. The daemon holds the session, so a
/// signed-out daemon and an empty organization arrive as different errors.
pub fn projects() -> Result<Vec<Project>, String> {
    let response = server("GET", "/api/v1/projects", BTreeMap::new(), None)?;
    if response.status != 200 {
        return Err(server_error(&response));
    }
    serde_json::from_str::<ProjectPage>(&response.body)
        .map(|page| page.items)
        .map_err(|error| format!("unreadable Project list: {error}"))
}

/// Creates a memory space: a Project in this Organization, which is what the
/// reader then works in. The Server names it and answers with the record.
pub fn create_project(name: &str, description: &str) -> Result<Project, String> {
    let body = serde_json::json!({ "name": name, "description": description }).to_string();
    // The Server deduplicates creation on this key, so a retry of one click is
    // one space rather than two.
    let mut headers = json_headers();
    headers.insert(
        "Idempotency-Key".to_owned(),
        uuid::Uuid::new_v4().to_string(),
    );
    let response = server("POST", "/api/v1/projects", headers, Some(body))?;
    if response.status != 200 && response.status != 201 {
        return Err(server_error(&response));
    }
    serde_json::from_str(&response.body).map_err(|error| format!("unreadable Project: {error}"))
}

/// Where this memory space keeps its guidelines on this machine, which is the
/// one thing about a Project that the daemon owns rather than the Server.
///
/// It is read and not written: the daemon's only call for it carries the session
/// as well, and the daemon deliberately does not hand the session back — macOS
/// reads this path for the same reason and no more.
pub fn memory_guidelines_path() -> Option<String> {
    client()
        .project_config()
        .ok()
        .and_then(|config| config.memory_guidelines_path)
}

/// Changes what a memory space is called, or what it says about itself.
///
/// The write carries the revision it was read at, which is what stops two
/// members' edits from silently replacing each other; the answer is the space
/// at its next revision.
pub fn update_project(
    project_id: &str,
    name: &str,
    description: &str,
    revision: i64,
) -> Result<Project, String> {
    let body = serde_json::json!({ "name": name, "description": description }).to_string();
    let mut headers = json_headers();
    headers.insert("If-Match".to_owned(), revision.to_string());
    let response = server(
        "PATCH",
        &format!("/api/v1/projects/{project_id}"),
        headers,
        Some(body),
    )?;
    if response.status != 200 {
        return Err(server_error(&response));
    }
    serde_json::from_str(&response.body).map_err(|error| format!("unreadable Project: {error}"))
}

fn read_ready_checkout(
    mut read: impl FnMut() -> Result<DaemonProjectCheckout, String>,
    sync: impl FnOnce() -> Result<(), String>,
) -> Result<DaemonProjectCheckout, String> {
    let checkout = read()?;
    if checkout.ready {
        return Ok(checkout);
    }
    sync()?;
    let checkout = read()?;
    if !checkout.ready {
        return Err("Project memory is not ready after synchronization. Try again.".into());
    }
    Ok(checkout)
}

#[cfg(test)]
mod checkout_tests {
    use super::*;
    use std::cell::Cell;

    fn snapshot(ready: bool) -> DaemonProjectCheckout {
        DaemonProjectCheckout {
            project_id: "project".into(),
            commit_id: ready.then(|| "published-commit".into()),
            ref_etag: None,
            commit_created_at: None,
            org_selection_revision: 0,
            selected_org_resource_ids: Vec::new(),
            resources: Vec::new(),
            ready,
        }
    }

    #[test]
    fn missing_cache_is_downloaded_before_it_is_displayed() {
        let downloaded = Cell::new(false);
        let result = read_ready_checkout(
            || Ok(snapshot(downloaded.get())),
            || {
                downloaded.set(true);
                Ok(())
            },
        )
        .unwrap();
        assert!(downloaded.get());
        assert!(result.ready);
        assert_eq!(result.commit_id.as_deref(), Some("published-commit"));
    }

    #[test]
    fn ready_empty_project_does_not_need_a_download() {
        let result = read_ready_checkout(
            || Ok(snapshot(true)),
            || panic!("an existing ready checkout must remain available offline"),
        )
        .unwrap();
        assert!(result.ready);
        assert!(result.resources.is_empty());
    }

    #[test]
    fn failed_download_is_an_error_instead_of_an_empty_project() {
        let error = read_ready_checkout(|| Ok(snapshot(false)), || Err("download failed".into()))
            .unwrap_err();
        assert_eq!(error, "download failed");
    }

    #[test]
    fn incomplete_download_is_never_displayed_as_an_empty_project() {
        assert!(read_ready_checkout(|| Ok(snapshot(false)), || Ok(())).is_err());
    }

    #[test]
    fn unreadable_cache_does_not_start_an_unrelated_retry() {
        let error = read_ready_checkout(
            || Err("daemon unavailable".into()),
            || panic!("propagate the read failure"),
        )
        .unwrap_err();
        assert_eq!(error, "daemon unavailable");
    }
}

/// Keeps the selected Project in the daemon's regular synchronization set.
/// This is a local configuration write; downloads run on the daemon's worker.
pub fn select_project(project_id: &str) -> Result<(), String> {
    client()
        .select_project(DaemonProjectSelectionRequest {
            project_id: project_id.to_owned(),
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Loads a Project on selection, downloading published content if needed.
/// The first download may need the Server, so this runs off the UI thread.
pub fn load_checkout(project_id: &str) -> Result<Checkout, String> {
    let checkout = read_ready_checkout(
        || read_project_checkout(project_id),
        || sync_project_channel(&client(), project_id, SyncRetryChannel::Commits),
    )?;
    checkout_documents(project_id, checkout)
}

/// Re-reads local content after an edit, without starting a network download.
pub fn checkout(project_id: &str) -> Result<Checkout, String> {
    let checkout = read_project_checkout(project_id)?;
    if !checkout.ready {
        return Err("Project memory has not been downloaded. Try again to load it.".into());
    }
    checkout_documents(project_id, checkout)
}

fn read_project_checkout(project_id: &str) -> Result<DaemonProjectCheckout, String> {
    client()
        .project_checkout(DaemonProjectCheckoutRequest {
            project_id: project_id.to_owned(),
        })
        .map_err(|error| error.to_string())
}

fn checkout_documents(
    project_id: &str,
    checkout: DaemonProjectCheckout,
) -> Result<Checkout, String> {
    let mut documents = memory_entries(checkout.resources);
    let drafts = drafts(project_id)?;
    // A document with a proposal is opened as the proposal has it, which is
    // what the macOS client's catalog does when it builds a Project's list.
    for draft in &drafts {
        let Some(target) = draft.target_id.as_deref() else {
            continue;
        };
        let Some(document) = documents
            .iter_mut()
            .find(|document| document.resource_id == target)
        else {
            continue;
        };
        if let Ok(detail) = client().get_draft(&draft.draft_id) {
            document.draft_content = proposed_text(&detail);
            document.draft_deleted = detail
                .operations
                .iter()
                .rev()
                .find_map(|op| {
                    if op.operation.delete.is_some() {
                        Some(true)
                    } else if op.operation.create.is_some() || op.operation.update.is_some() {
                        Some(false)
                    } else {
                        None
                    }
                })
                .unwrap_or(false);
            if let Some(path) = detail.operations.iter().rev().find_map(|op| {
                op.operation
                    .rename
                    .as_ref()
                    .map(|r| r.new_path.clone())
                    .or_else(|| op.operation.create.as_ref().map(|c| c.path.clone()))
            }) {
                document.path = path;
            }
        }
    }
    // A draft that creates a file proposes a document the Project does not hold
    // yet, and that document is a row like any other: the reader can open it,
    // edit it, rename it, ask for a Review of it and throw it away. Nothing is
    // published behind it, which is what `published` says.
    for draft in &drafts {
        if draft
            .target_id
            .as_ref()
            .is_some_and(|id| documents.iter().any(|d| &d.resource_id == id))
        {
            continue;
        }
        let Some(path) = draft.path.clone() else {
            continue;
        };
        if documents.iter().any(|document| document.path == path) {
            continue;
        }
        let detail = client().get_draft(&draft.draft_id).ok();
        let proposed = detail.as_ref().and_then(proposed_content);
        documents.push(MemoryDocument {
            resource_id: draft.draft_id.clone(),
            published: false,
            is_directory: proposed.is_some_and(|content| content.is_directory),
            org_owned: draft.scope == DaemonDraftScope::Org,
            draft_deleted: false,
            path,
            content: String::new(),
            draft_content: proposed.map(|content| content.content.clone()),
        });
    }
    documents.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(Checkout {
        project_id: checkout.project_id,
        commit_id: checkout.commit_id,
        documents,
    })
}

/// Keep directory records out of the document-only editor. Files beneath
/// them still produce the inferred tree folders supported by this client.
fn memory_entries(resources: Vec<DaemonProjectCheckoutResource>) -> Vec<MemoryDocument> {
    let mut documents: Vec<_> = resources
        .into_iter()
        .map(|resource| MemoryDocument {
            resource_id: resource.resource_id,
            published: true,
            is_directory: resource.content.is_directory,
            org_owned: resource.scope == DaemonDraftScope::Org,
            draft_deleted: false,
            path: resource.path,
            content: resource.content.content,
            draft_content: None,
        })
        .collect();
    documents.sort_by(|left, right| left.path.cmp(&right.path));
    documents
}

/// Hands the daemon a session. The client never keeps one: it holds the tokens
/// for as long as it takes to pass them over.
pub fn install_session(
    server_url: &str,
    access_token: &str,
    refresh_token: Option<&str>,
) -> Result<(), String> {
    client()
        .replace_project_config(clumsiesd::DaemonProjectConfigUpdateRequest {
            server_url: server_url.to_owned(),
            project_id: None,
            memory_guidelines_path: None,
            access_token: Some(access_token.to_owned()),
            refresh_token: refresh_token.map(str::to_owned),
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Ends the session.
///
/// The Server is told to revoke it, which is best effort: a session this machine
/// cannot revoke is still a session this machine stops using, and macOS forgets
/// it either way. Then the daemon — the only party that ever held the tokens —
/// is left with a Server address and nothing else.
pub fn sign_out() -> Result<(), String> {
    // Account lookup may fail precisely when the user needs to sign out.
    // The daemon's configured origin does not depend on a valid session.
    let daemon = client();
    let server_url = daemon
        .health()
        .map_err(|error| error.to_string())?
        .server_url;
    match server("DELETE", "/api/v1/auth/session", BTreeMap::new(), None) {
        Ok(response) if response.status == 204 || response.status == 200 => {}
        Ok(response) => crate::logging::error(&format!(
            "the Server would not revoke the session: HTTP {}",
            response.status
        )),
        Err(error) => {
            crate::logging::error(&format!("could not reach the Server to revoke: {error}"))
        }
    }
    daemon
        .replace_project_config(clumsiesd::DaemonProjectConfigUpdateRequest {
            server_url,
            project_id: None,
            memory_guidelines_path: None,
            access_token: None,
            refresh_token: None,
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Who the daemon's session belongs to: the identity, the Organization it is a
/// member of, and what its role lets it do.
#[derive(Clone)]
pub struct Account {
    pub user: AccountUser,
    pub organization: String,
    pub capabilities: Vec<String>,
    pub project_roles: BTreeMap<String, String>,
}

#[derive(Clone, Deserialize)]
pub struct AccountUser {
    pub user_id: String,
    pub email: Option<String>,
    pub username: Option<String>,
    pub display_name: Option<String>,
}

impl AccountUser {
    /// What a menu calls this account — macOS's `identityLabel`.
    pub fn identity_label(&self) -> &str {
        self.display_name
            .as_deref()
            .or(self.username.as_deref())
            .or(self.email.as_deref())
            .unwrap_or(&self.user_id)
    }

    /// What this account signs in with — macOS's `loginLabel`.
    pub fn login_label(&self) -> &str {
        self.username
            .as_deref()
            .or(self.email.as_deref())
            .unwrap_or(&self.user_id)
    }
}

#[derive(Deserialize)]
struct MeResponse {
    user: AccountUser,
    org: OrgName,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    projects: Vec<ProjectRole>,
}

#[derive(Deserialize)]
struct ProjectRole {
    project_id: String,
    role: String,
}

#[derive(Deserialize)]
struct OrgName {
    name: String,
}

/// Reads the account the daemon's session belongs to. The daemon holds the
/// session, so this is its Server proxy answering.
pub fn account() -> Result<Account, String> {
    let response = server("GET", "/api/v1/me", BTreeMap::new(), None)?;
    if response.status != 200 {
        return Err(server_error(&response));
    }
    let me: MeResponse = serde_json::from_str(&response.body)
        .map_err(|error| format!("unreadable account: {error}"))?;
    Ok(Account {
        user: me.user,
        organization: me.org.name,
        capabilities: me.capabilities,
        project_roles: me
            .projects
            .into_iter()
            .map(|p| (p.project_id, p.role))
            .collect(),
    })
}

/// What an account signs in with, and whether it has a local password — macOS's
/// `AccountCredentialStatus`.
#[derive(Clone, Deserialize)]
pub struct Credentials {
    /// The local username, when the account has one.
    pub username: Option<String>,
    /// Whether a local password has been established.
    pub password_set: bool,
    /// The email the identity provider recorded when the account was connected.
    pub oidc_email: Option<String>,
}

/// Reads what the signed-in account signs in with. The daemon holds the
/// session, so this is its Server proxy answering.
pub fn credentials() -> Result<Credentials, String> {
    let response = server("GET", "/api/v1/auth/credentials", BTreeMap::new(), None)?;
    if response.status != 200 {
        return Err(server_error(&response));
    }
    serde_json::from_str(&response.body).map_err(|error| format!("unreadable credentials: {error}"))
}

/// Sets or changes the local password.
///
/// The Server answers with a fresh session, because changing a password signs
/// out every other session — this one included — so the caller has to hand the
/// new tokens to the daemon before the old ones stop working.
pub fn change_password(
    username: Option<&str>,
    current_password: Option<&str>,
    password: &str,
) -> Result<crate::sign_in::Session, String> {
    let body = serde_json::json!({
        "username": username,
        "current_password": current_password,
        "password": password,
    })
    .to_string();
    let response = server("POST", "/api/v1/auth/password", json_headers(), Some(body))?;
    if response.status != 200 {
        return Err(server_error(&response));
    }
    serde_json::from_str(&response.body).map_err(|error| format!("unreadable session: {error}"))
}

/// Connects an identity provider to the account that is signed in — macOS's
/// `bindOIDC`. The authorization is opened through the daemon's proxy, because
/// the daemon holds the session and an authenticated call is its to make; the
/// browser round trip and the token exchange that follow are the sign-in flow's
/// own, and they answer with a session for the same reason a password change
/// does.
pub fn bind_identity(current_password: Option<&str>) -> Result<crate::sign_in::Session, String> {
    let origin = configured_server_url()
        .ok_or_else(|| "the daemon is not pointed at a Server".to_owned())?;
    let authorization = crate::sign_in::begin_authorization()?;
    let body = serde_json::json!({
        "authorization": {
            "client_kind": "desktop",
            "redirect_uri": authorization.redirect.uri,
            "state": authorization.state,
            "code_challenge": authorization.challenge,
            "code_challenge_method": "S256",
        },
        "current_password": current_password,
    })
    .to_string();
    let response = server(
        "POST",
        "/api/v1/auth/oidc-bindings",
        json_headers(),
        Some(body),
    )?;
    if response.status != 200 {
        return Err(server_error(&response));
    }
    #[derive(Deserialize)]
    struct Binding {
        authorization_url: String,
    }
    let binding: Binding = serde_json::from_str(&response.body)
        .map_err(|error| format!("unreadable binding: {error}"))?;
    crate::sign_in::finish_authorization(
        &crate::sign_in::client()?,
        &origin,
        authorization.redirect,
        &binding.authorization_url,
        &authorization.verifier,
        &authorization.state,
    )
}

/// The headers a JSON body travels with, which the Server needs to read it.
fn json_headers() -> BTreeMap<String, String> {
    BTreeMap::from([("content-type".to_owned(), "application/json".to_owned())])
}

/// Whether the daemon or Server requires a new sign-in.
///
/// Missing credentials and a rejected/expired session both return 401. Keep
/// the legacy missing-token message for older daemons; transport and permission
/// failures must not be treated as sign-out.
pub fn missing_session(error: &str) -> bool {
    error.contains("access_token is required")
        || error.contains("Server request failed with status 401:")
        || error == "the Server answered HTTP 401"
        || error.starts_with("the Server answered HTTP 401:")
}

/// The Server the daemon is configured for, which is what the sign-in form
/// starts from.
pub fn configured_server_url() -> Option<String> {
    match client().health() {
        Ok(health) => Some(health.server_url),
        Err(_) => None,
    }
}

/// Proposes a new document at a path, which is how a Project's Memory grows.
/// The file exists once the Review that carries it is merged, which is what
/// makes a proposal safe to write.
pub fn create_document(
    project_id: &str,
    base_commit_id: Option<&str>,
    path: &str,
    content: &str,
) -> Result<DaemonDraftOperationResponse, String> {
    create_memory_entry(project_id, base_commit_id, path, content, false)
}

pub fn create_memory_entry(
    project_id: &str,
    base_commit_id: Option<&str>,
    path: &str,
    content: &str,
    is_directory: bool,
) -> Result<DaemonDraftOperationResponse, String> {
    draft_operation(&DaemonDraftOperationRequest {
        draft_id: None,
        base_commit_id: base_commit_id.map(str::to_owned),
        project_id: project_id.to_owned(),
        scope: DaemonDraftScope::Project,
        resource: DaemonDraftResourceKind::Memory,
        op: DaemonDraftOperation {
            create: Some(DaemonCreateDraftOperation {
                path: path.to_owned(),
                content: DaemonDraftContent {
                    is_directory,
                    org_source: None,
                    description: None,
                    content: content.to_owned(),
                },
                description: None,
            }),
            update: None,
            rename: None,
            delete: None,
            discard: None,
        },
        source: Some(DaemonDraftOperationSource::Desktop),
    })
}

/// One document edit, in the shape the daemon's draft operation takes. It owns
/// its text because the edit outlives the keystroke that produced it: the
/// Review sheet holds one while it waits for the network.
#[derive(Clone)]
pub struct DocumentEdit {
    pub project_id: String,
    /// The draft's own base when one is already open; the Project ref otherwise.
    pub base_commit_id: Option<String>,
    pub draft_id: Option<String>,
    /// The Memory resource, or — for a document that only exists as a proposal
    /// — the draft that proposes it.
    pub resource_id: String,
    /// Whether the Project already holds this document. A proposal is written
    /// with the create operation, and has nothing to delete.
    pub published: bool,
    pub is_directory: bool,
    pub org_owned: bool,
    /// Where the document is, which is the path the create operation names.
    pub path: String,
    pub content: String,
}

/// Proposes a new path for a document. A rename is a draft like any other, so
/// it is reviewed and published the way an edit is.
pub fn rename_document(
    document: &DocumentEdit,
    new_path: &str,
) -> Result<DaemonDraftOperationResponse, String> {
    // A proposal is renamed by proposing to create it somewhere else: there is
    // no resource for a rename to move, and the daemon records the path the
    // file would be created at.
    let op = DaemonDraftOperation {
        create: (!document.published).then(|| DaemonCreateDraftOperation {
            path: new_path.to_owned(),
            content: content_of(document),
            description: None,
        }),
        update: None,
        rename: document.published.then(|| DaemonRenameDraftOperation {
            id: document.resource_id.clone(),
            new_path: new_path.to_owned(),
            description: None,
        }),
        delete: None,
        discard: None,
    };
    draft_operation(&DaemonDraftOperationRequest {
        draft_id: document.draft_id.clone(),
        base_commit_id: document.base_commit_id.clone(),
        project_id: document.project_id.clone(),
        scope: if document.org_owned {
            DaemonDraftScope::Org
        } else {
            DaemonDraftScope::Project
        },
        resource: DaemonDraftResourceKind::Memory,
        op,
        source: Some(DaemonDraftOperationSource::Desktop),
    })
}

/// Proposes that a document be deleted. The deletion takes effect when the
/// Review carrying it is merged, which is what makes it safe to offer.
pub fn delete_document(document: &DocumentEdit) -> Result<DaemonDraftOperationResponse, String> {
    draft_operation(&DaemonDraftOperationRequest {
        draft_id: document.draft_id.clone(),
        base_commit_id: document.base_commit_id.clone(),
        project_id: document.project_id.clone(),
        scope: if document.org_owned {
            DaemonDraftScope::Org
        } else {
            DaemonDraftScope::Project
        },
        resource: DaemonDraftResourceKind::Memory,
        op: DaemonDraftOperation {
            create: None,
            update: None,
            rename: None,
            delete: Some(DaemonDeleteDraftOperation {
                id: document.resource_id.clone(),
                description: None,
            }),
            discard: None,
        },
        source: Some(DaemonDraftOperationSource::Desktop),
    })
}

/// Moves this Project's Memory back to the standard location.
pub fn reset_project_storage(project_id: &str, revision: i64) -> Result<(), String> {
    client()
        .reset_project_storage(DaemonProjectStorageResetRequest {
            project_id: project_id.to_owned(),
            expected_location_revision: revision,
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Builds again what the Project's cache holds. Drafts and settings stay.
pub fn clear_project_cache(project_id: &str, revision: i64) -> Result<(), String> {
    client()
        .clear_project_cache(DaemonProjectCacheClearRequest {
            project_id: project_id.to_owned(),
            expected_location_revision: revision,
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Asks the daemon to sync this Project's drafts now instead of on its next
/// tick, which is the one command about the engine's own work.
pub fn sync_now(project_id: &str) -> Result<(), String> {
    nudge_drafts(&client(), project_id)
}

/// Throws a proposal away, which is what a reader does with an edit they no
/// longer want. The published document is untouched.
pub fn discard_draft(
    project_id: &str,
    draft_id: &str,
    resource_id: &str,
) -> Result<DaemonDraftOperationResponse, String> {
    let detail = client().get_draft(draft_id).map_err(|e| e.to_string())?;
    let request = DaemonDraftOperationRequest {
        draft_id: Some(draft_id.to_owned()),
        base_commit_id: None,
        // The daemon resolves a draft by its id and checks that it belongs to
        // the Project the request names, so an empty one is refused.
        project_id: project_id.to_owned(),
        scope: detail.draft.scope,
        resource: DaemonDraftResourceKind::Memory,
        op: DaemonDraftOperation {
            create: None,
            update: None,
            rename: None,
            delete: None,
            discard: Some(DaemonDiscardDraftOperation {
                id: resource_id.to_owned(),
            }),
        },
        source: Some(DaemonDraftOperationSource::Desktop),
    };
    draft_operation(&request)
}

/// One draft operation through the daemon, which queues it and uploads behind
/// it. The desktop alias is the one that carries a read-write session.
fn draft_operation(
    request: &DaemonDraftOperationRequest,
) -> Result<DaemonDraftOperationResponse, String> {
    let payload = serde_json::to_value(request)
        .map_err(|error| format!("unreadable draft operation: {error}"))?;
    client()
        .call(DaemonIpcRequest::new(
            "desktop_store_draft_operation",
            payload,
        ))
        .map_err(|error| error.to_string())?
        .into_payload()
        .map_err(|error| error.to_string())
}

/// Writes a document's new text as a draft operation.
///
/// The daemon queues the operation and uploads it in the background, which is
/// why this returns as soon as it is stored: the caller waits separately with
/// `wait_for_upload` when it needs the Server to have seen the draft. The
/// operation updates the document's resource identity, which is the same
/// operation the macOS client stores from its editor.
///
/// The method is the desktop alias and not the plain store_draft_operation: the
/// daemon reserves that spelling for Agent protocol proxies, which prove their
/// identity, and refuses a request that arrives without one.
pub fn store_document(edit: &DocumentEdit) -> Result<DaemonDraftOperationResponse, String> {
    // A document that only exists as a proposal is written with the create
    // operation: there is no resource yet for an update to name, and the path
    // is what says which file the draft proposes. The daemon keeps one draft
    // per path, so writing again is what a further edit to a proposal is.
    let op = DaemonDraftOperation {
        create: (!edit.published).then(|| DaemonCreateDraftOperation {
            path: edit.path.clone(),
            content: content_of(edit),
            description: None,
        }),
        update: edit.published.then(|| {
            DaemonUpdateDraftOperation::Content(DaemonContentDraftUpdate {
                id: edit.resource_id.clone(),
                content: content_of(edit),
                description: None,
            })
        }),
        rename: None,
        delete: None,
        discard: None,
    };
    draft_operation(&DaemonDraftOperationRequest {
        draft_id: edit.draft_id.clone(),
        base_commit_id: edit.base_commit_id.clone(),
        project_id: edit.project_id.clone(),
        scope: if edit.org_owned {
            DaemonDraftScope::Org
        } else {
            DaemonDraftScope::Project
        },
        resource: DaemonDraftResourceKind::Memory,
        op,
        source: Some(DaemonDraftOperationSource::Desktop),
    })
}

/// The document's text as a draft carries it.
fn content_of(edit: &DocumentEdit) -> DaemonDraftContent {
    DaemonDraftContent {
        is_directory: edit.is_directory,
        org_source: None,
        description: None,
        content: edit.content.clone(),
    }
}

/// One edit, or several, through to a Review: each is stored when the caller
/// still holds text the engine has not accepted, each is waited for until the
/// daemon has uploaded its draft, and one Review then names every one of them —
/// which is what the Server's request is shaped for, and what macOS does with a
/// folder's worth of drafts.
///
/// The steps are one function because the last two are only meaningful on the
/// result of the first: the Server identifies a draft by the identity the upload
/// assigns, and a Review cannot name a draft the Server has not seen.
pub fn submit_documents_review(
    edits: &[DocumentEdit],
    store: bool,
    title: &str,
    description: &str,
) -> Result<Review, String> {
    if edits.is_empty() {
        return Err("there is nothing to review".to_owned());
    }
    let mut drafts = Vec::new();
    for edit in edits {
        let draft_id = if store {
            store_document(edit)?.draft_id
        } else {
            edit.draft_id
                .clone()
                .ok_or_else(|| "this document has no draft to review yet".to_owned())?
        };
        drafts.push(wait_for_upload(&draft_id)?);
    }
    request_reviews(&drafts, title, description)
}

/// The text a draft proposes for its document, taken from the last full content
/// it holds — whether that is the update that rewrote a published document or
/// the create that would write a new one. Either way the operation carries the
/// whole document, which is what this client stores.
fn proposed_content(detail: &DaemonDraftDetail) -> Option<&DaemonDraftContent> {
    detail.operations.iter().rev().find_map(|operation| {
        operation
            .operation
            .update
            .as_ref()
            .and_then(|update| update.content())
            .or_else(|| {
                operation
                    .operation
                    .create
                    .as_ref()
                    .map(|create| &create.content)
            })
    })
}

fn proposed_text(detail: &DaemonDraftDetail) -> Option<String> {
    proposed_content(detail).map(|content| content.content.clone())
}

/// The drafts of one Project that are still proposals: an open one takes
/// edits, a submitted one waits for a decision, and both describe what the
/// Project would hold next. A merged or discarded draft describes nothing.
///
/// The daemon lists drafts for every Project, so the caller's Project is
/// selected here.
pub fn drafts(project_id: &str) -> Result<Vec<DaemonDraftSummary>, String> {
    let response = client()
        .list_drafts(DaemonDraftListQuery {
            limit: Some(200),
            ..Default::default()
        })
        .map_err(|error| error.to_string())?;
    Ok(response
        .items
        .into_iter()
        .filter(|draft| draft.project_id == project_id)
        .filter(|draft| {
            !matches!(
                draft.status,
                DaemonLocalDraftStatus::Merged | DaemonLocalDraftStatus::Discarded
            )
        })
        .collect())
}

/// Waits until the Server has the draft and nothing is left to upload.
///
/// This is the barrier the macOS client calls
/// `synchronizedDraftForReconciliation`: it asks the daemon, nudges the drafts
/// channel once if nothing has moved, and gives up after `UPLOAD_TIMEOUT`.
/// A Review cannot be requested before it passes, because the Server identifies
/// a draft by the identity it assigns on upload.
pub fn wait_for_upload(draft_id: &str) -> Result<DaemonDraftSummary, String> {
    let client = client();
    let deadline = Instant::now() + UPLOAD_TIMEOUT;
    let mut nudged = false;
    loop {
        let detail = client
            .get_draft(draft_id)
            .map_err(|error| error.to_string())?;
        if let Some(failed) = detail
            .operations
            .iter()
            .rev()
            .find(|operation| operation.sync_status == DraftOperationSyncStatus::Failed)
        {
            return Err(failed
                .last_error
                .clone()
                .unwrap_or_else(|| "the engine could not upload this draft".to_owned()));
        }
        let uploaded = detail.draft.server_draft_id.is_some()
            && detail.draft.pending_operation_count == 0
            && detail.draft.failed_operation_count == 0;
        if uploaded {
            return Ok(detail.draft);
        }
        if Instant::now() >= deadline {
            return Err(
                "the engine is still uploading this draft; try again in a moment".to_owned(),
            );
        }
        if !nudged {
            nudged = true;
            nudge_drafts(&client, &detail.draft.project_id)?;
        }
        sleep(UPLOAD_POLL);
    }
}

/// Asks the daemon to sync the drafts channel now instead of on its next tick.
fn nudge_drafts(client: &DaemonIpcClient, project_id: &str) -> Result<(), String> {
    sync_project_channel(client, project_id, SyncRetryChannel::Drafts)
}

/// Retries only the requested channel; loading a checkout must not retry uploads.
fn sync_project_channel(
    client: &DaemonIpcClient,
    project_id: &str,
    channel: SyncRetryChannel,
) -> Result<(), String> {
    let payload = serde_json::to_value(DaemonProjectSyncRetryRequest {
        project_id: project_id.to_owned(),
        channel,
    })
    .map_err(|error| format!("unreadable retry request: {error}"))?;
    let response = client
        .call(DaemonIpcRequest::new("project_retry_sync", payload))
        .map_err(|error| error.to_string())?;
    let _: DaemonRetryResponse = response.into_payload().map_err(|error| error.to_string())?;
    Ok(())
}

/// Asks the Server to turn an uploaded draft into a Review.
///
/// The Server guards the Project ref with `If-Match`, so the request carries the
/// ref the draft was rebased onto; a ref that moved under the draft is refused
/// rather than reviewed against the wrong base.
pub fn request_reviews(
    drafts: &[DaemonDraftSummary],
    title: &str,
    description: &str,
) -> Result<Review, String> {
    let mut named = Vec::new();
    for draft in drafts {
        let Some(server_draft_id) = draft.server_draft_id.clone() else {
            return Err("the engine has not uploaded this draft yet".to_owned());
        };
        named.push(serde_json::json!({
            "draft_id": server_draft_id,
            "expected_draft_version": draft.server_version,
        }));
    }
    // Every draft of one selection was written against the same checkout, so the
    // first one names the commit the whole request is decided against.
    let base = drafts
        .first()
        .and_then(|draft| draft.current_commit_id.as_deref())
        .unwrap_or("ref-none");
    let mut headers = BTreeMap::new();
    headers.insert("If-Match".to_owned(), format!("\"{base}\""));
    headers.insert("content-type".to_owned(), "application/json".to_owned());
    let body = serde_json::json!({
        "drafts": named,
        "title": title,
        "description": description,
    });
    let response = server("POST", "/api/v1/reviews", headers, Some(body.to_string()))?;
    if response.status != 200 {
        return Err(server_error(&response));
    }
    let detail: ReviewDetailResponse = serde_json::from_str(&response.body)
        .map_err(|error| format!("unreadable Review: {error}"))?;
    Ok(detail.review)
}

/// The Reviews of one Project, newest first, which is the order the Server
/// answers in and the order macOS lists them.
pub fn reviews(project_id: &str) -> Result<Vec<Review>, String> {
    let response = server(
        "GET",
        &format!("/api/v1/reviews?project_id={project_id}"),
        BTreeMap::new(),
        None,
    )?;
    if response.status != 200 {
        return Err(server_error(&response));
    }
    serde_json::from_str::<ReviewPage>(&response.body)
        .map(|page| page.items)
        .map_err(|error| format!("unreadable Review list: {error}"))
}

/// One Review with its proposals and its discussion.
pub fn review(review_id: &str) -> Result<ReviewDetail, String> {
    let response = server(
        "GET",
        &format!("/api/v1/reviews/{review_id}"),
        BTreeMap::new(),
        None,
    )?;
    if response.status != 200 {
        return Err(server_error(&response));
    }
    let response: ReviewDetailResponse = serde_json::from_str(&response.body)
        .map_err(|error| format!("unreadable Review: {error}"))?;
    let documents = reviewed_documents(response.draft, response.operations, response.drafts);
    Ok(ReviewDetail {
        review: response.review,
        documents,
    })
}

/// Records an approval or a rejection. The note is what the author will read,
/// so an empty one is sent as nothing rather than as an empty string.
pub fn decide_review(review: &Review, decision: ReviewStatus, note: &str) -> Result<(), String> {
    let mut headers = BTreeMap::new();
    headers.insert("content-type".to_owned(), "application/json".to_owned());
    let decision = match decision {
        ReviewStatus::Approved => "approved",
        ReviewStatus::Rejected => "rejected",
        other => return Err(format!("a Review cannot be decided as {}", other.label())),
    };
    let body = serde_json::json!({
        "decision": decision,
        "expected_review_version": review.version,
        "body": (!note.trim().is_empty()).then_some(note),
    });
    let response = server(
        "POST",
        &format!("/api/v1/reviews/{}/decisions", review.review_id),
        headers,
        Some(body.to_string()),
    )?;
    if response.status != 200 {
        return Err(server_error(&response));
    }
    Ok(())
}

/// Publishes an approved Review. The reference it publishes to must be the one
/// the approval covered, which is what the If-Match header says.
pub fn merge_review(review: &Review) -> Result<(), String> {
    let mut headers = BTreeMap::new();
    headers.insert(
        "If-Match".to_owned(),
        format!(
            "\"{}\"",
            review
                .coordination
                .current_commit_id
                .as_deref()
                .unwrap_or("ref-none")
        ),
    );
    headers.insert("content-type".to_owned(), "application/json".to_owned());
    let body = serde_json::json!({ "expected_review_version": review.version });
    let response = server(
        "POST",
        &format!("/api/v1/reviews/{}/merges", review.review_id),
        headers,
        Some(body.to_string()),
    )?;
    if response.status != 200 {
        return Err(server_error(&response));
    }
    Ok(())
}

/// What the Server answers for a list of Reviews.
#[derive(Deserialize)]
struct ReviewPage {
    items: Vec<Review>,
}

/// How many calendar days a Dashboard read covers. macOS offers the same three
/// in its toolbar picker, and the Server accepts nothing else.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Week,
    Month,
    Quarter,
}

impl Period {
    pub const ALL: [Period; 3] = [Period::Week, Period::Month, Period::Quarter];

    pub fn days(self) -> u32 {
        match self {
            Period::Week => 7,
            Period::Month => 30,
            Period::Quarter => 90,
        }
    }

    /// What the picker calls it, which is macOS's own label.
    pub fn label(self) -> &'static str {
        match self {
            Period::Week => "7 days",
            Period::Month => "30 days",
            Period::Quarter => "90 days",
        }
    }
}

/// The Server's half of the Dashboard: the published inventory and its history,
/// bucketed in the reader's own time zone.
///
/// Read from `MemoryStatistics` in `crates/server/src/app/memory/dto.rs`, which
/// carries more than this client draws: the change buckets and the deleted
/// count are left out here, because no screen asks for them.
#[derive(Clone, Debug, Deserialize)]
pub struct MemoryStatistics {
    /// The Server's clock when it answered, which the engine's telemetry is
    /// measured against.
    pub generated_at: i64,
    /// Local calendar boundaries, including the exclusive one after the last
    /// day. The engine validates their shape and refuses anything else.
    pub day_bounds: Vec<i64>,
    /// The starts of the last 7, 30 and 90 days, which the engine groups its
    /// most-recent retrieval by.
    pub recency_starts: Vec<i64>,
    /// The Projects this answer covers — one, for a Project's own statistics.
    pub project_ids: Vec<String>,
    /// What the Project publishes today, which is what retrieval is measured
    /// against.
    pub resources: Vec<StatisticsResource>,
    pub memory_count: usize,
    /// Distinct documents added and edited within the period.
    pub added_count: usize,
    pub updated_count: usize,
    /// One entry per day of the period, in order.
    pub days: Vec<InventoryDay>,
    /// Drafts the Server knows about: open or conflicted, and submitted.
    pub open_drafts: i64,
    pub submitted_drafts: i64,
}

/// One published document, as the Server's statistics name it. The id is the
/// Memory resource, which is what the engine's telemetry counts and what a
/// Dashboard row opens.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StatisticsResource {
    pub id: String,
    pub title: String,
    pub path: String,
}

/// One day's closing inventory. `None` predates the first retained snapshot,
/// which is not the same as a day that held nothing.
#[derive(Clone, Debug, Deserialize)]
pub struct InventoryDay {
    pub date: i64,
    pub memory_count: Option<usize>,
}

/// The engine's half of the Dashboard: retrieval telemetry retained on this
/// machine. Read from `DashboardRetrievalStatistics` in
/// `crates/clumsiesd/src/dashboard.rs`, whose nested types the daemon does not
/// export; the field names here are that struct's, and a test below pins them.
#[derive(Clone, Debug, Deserialize)]
pub struct RetrievalStatistics {
    /// Completed requests in the period.
    pub retrievals: usize,
    /// Distinct published documents those requests returned.
    pub recalled_count: usize,
    /// The share of the current inventory that was retrieved at least once.
    pub coverage: f64,
    /// Only the days with retained records are sent; a day nobody asked on is
    /// absent rather than zero.
    pub days: Vec<RetrievalDay>,
    /// Directory counts and their coverage of the current inventory.
    pub directories: Vec<DashboardBar>,
    /// The most frequently retrieved documents, six at most.
    pub top_resources: Vec<DashboardBar>,
    /// The oldest retained request within 90 days, when there is one. A flat
    /// week can mean a week nobody asked in, or a week this machine never saw.
    pub history_start: Option<i64>,
    /// How many requests per Project this machine keeps. Counts are local
    /// history, never a claim about anyone else's.
    pub retention_per_project: i64,
    /// Fragment delta actions, absent on an engine too old to record them.
    #[serde(default)]
    pub delta: Option<DeltaStatistics>,
    /// Completed agent runs seen in local session logs, when the engine reads
    /// them.
    #[serde(default)]
    pub agent_usage: Option<AgentUsage>,
}

/// One local day's completed retrieval, by outcome.
#[derive(Clone, Debug, Deserialize)]
pub struct RetrievalDay {
    pub date: i64,
    /// Successful requests that returned or reused content.
    pub returned: usize,
    /// Successful requests that returned nothing.
    pub empty: usize,
    /// Requests that failed.
    pub failed: usize,
}

/// A bar of a horizontal chart: a directory, a retrieved document, or one of
/// the recency groups. `total` is the inventory a coverage bar is measured
/// against, and is absent from a bar that counts requests.
#[derive(Clone, Debug, Deserialize)]
pub struct DashboardBar {
    pub id: String,
    pub label: String,
    pub value: usize,
    pub total: Option<usize>,
}

/// Delta telemetry counts fragments rather than documents or requests, which is
/// why its rate is the ratio of three of its own actions rather than a share of
/// the inventory.
#[derive(Clone, Debug, Deserialize)]
pub struct DeltaStatistics {
    /// Completed requests carrying a state token, including rejected ones.
    pub with_state: usize,
    pub days: Vec<DeltaDay>,
}

impl DeltaStatistics {
    pub fn added(&self) -> usize {
        self.days.iter().map(|day| day.added).sum()
    }

    pub fn replaced(&self) -> usize {
        self.days.iter().map(|day| day.replaced).sum()
    }

    pub fn reused(&self) -> usize {
        self.days.iter().map(|day| day.reused).sum()
    }

    pub fn unknown(&self) -> usize {
        self.days.iter().map(|day| day.unknown).sum()
    }

    /// Fragments whose action was recorded; the ones without one are excluded
    /// from the ratio rather than counted as reuse.
    pub fn total(&self) -> usize {
        self.added() + self.replaced() + self.reused()
    }

    /// Reuse / (add + replace + reuse), which is macOS's own ratio.
    pub fn reuse_rate(&self) -> Option<f64> {
        (self.total() > 0).then(|| self.reused() as f64 / self.total() as f64)
    }
}

/// One day's fragment actions.
#[derive(Clone, Debug, Deserialize)]
pub struct DeltaDay {
    pub date: i64,
    pub added: usize,
    pub replaced: usize,
    pub reused: usize,
    pub unknown: usize,
}

impl DeltaDay {
    pub fn total(&self) -> usize {
        self.added + self.replaced + self.reused
    }
}

/// Completed local agent runs, and whether each called Memory.
#[derive(Clone, Debug, Deserialize)]
pub struct AgentUsage {
    pub days: Vec<AgentUsageDay>,
    /// Session logs the engine could not read, and ones it does not
    /// understand. Both are named so a rate is never read as the whole truth.
    pub unreadable_sessions: usize,
    pub unsupported_sessions: usize,
}

impl AgentUsage {
    pub fn with_memory(&self) -> usize {
        self.days.iter().map(|day| day.with_memory).sum()
    }

    pub fn total(&self) -> usize {
        self.days.iter().map(AgentUsageDay::total).sum()
    }

    pub fn usage_rate(&self) -> Option<f64> {
        (self.total() > 0).then(|| self.with_memory() as f64 / self.total() as f64)
    }

    /// Sessions left out of both counts, which the panel says out loud.
    pub fn excluded_sessions(&self) -> usize {
        self.unreadable_sessions + self.unsupported_sessions
    }
}

/// One day's agent runs.
#[derive(Clone, Debug, Deserialize)]
pub struct AgentUsageDay {
    pub date: i64,
    pub with_memory: usize,
    pub without_memory: usize,
}

impl AgentUsageDay {
    pub fn total(&self) -> usize {
        self.with_memory + self.without_memory
    }
}

/// One read of the Dashboard: what the Project publishes over a period, what
/// the engine retained of its retrieval, and whether the Server's half came
/// from the daemon's cache rather than from the Server just now.
///
/// The two halves are not joined here. Each panel draws one of them against its
/// own days, which is what macOS's `DashboardSummary` does with the pair: the
/// join it computes is not read by any of its charts either.
pub struct DashboardSnapshot {
    pub period: Period,
    pub memory: MemoryStatistics,
    pub retrieval: RetrievalStatistics,
    /// The daemon answers from its cache when the Server cannot be reached, and
    /// says so in a header. A cached number is worth showing, and worth saying.
    pub stale: bool,
    /// Whether this came from a Dev Instance's fixture rather than from the
    /// Server and the engine. macOS badges the same file "Demo data", because a
    /// sample is not this Project's own history.
    pub demo: bool,
}

impl DashboardSnapshot {
    /// The documents this snapshot names, which is how a Dashboard row turns a
    /// resource id back into the path Memory opens.
    pub fn path_for(&self, resource_id: &str) -> Option<&str> {
        self.memory
            .resources
            .iter()
            .find(|resource| resource.id == resource_id)
            .map(|resource| resource.path.as_str())
    }

    /// The largest inventory of the period, which is the scale the growth chart
    /// draws against.
    pub fn peak_inventory(&self) -> usize {
        self.memory
            .days
            .iter()
            .filter_map(|day| day.memory_count)
            .max()
            .unwrap_or(0)
    }
}

/// Reads both halves of the Dashboard for one Project.
///
/// A Dev Instance's fixture is read first when there is one, because a screen
/// whose numbers take a month to accumulate cannot otherwise be looked at. The
/// live read is the Server and the engine: the Server owns the published
/// inventory and the calendar, so it is asked for the statistics of the period
/// in the reader's time zone, and the engine owns the retrieval telemetry, so it
/// is asked with the Server's own boundaries and both halves describe the same
/// days. Nothing here writes anything.
pub fn dashboard(project_id: &str, period: Period) -> Result<DashboardSnapshot, String> {
    if let Some(snapshot) = demo_snapshot(project_id, period) {
        return Ok(snapshot);
    }
    let response = server(
        "GET",
        &statistics_path(project_id, period, &crate::timestamps::time_zone()),
        BTreeMap::new(),
        None,
    )?;
    if response.status != 200 {
        return Err(server_error(&response));
    }
    let memory: MemoryStatistics = serde_json::from_str(&response.body)
        .map_err(|error| format!("unreadable Project statistics: {error}"))?;
    // The daemon keeps the Server's last answer and serves it when the Server
    // cannot be reached, marking it with this header. The caller reports it
    // rather than passing a kept number off as this minute's.
    let stale = response.headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("x-clumsies-cache") && value.eq_ignore_ascii_case("stale")
    });
    // The engine's own call takes exactly the boundaries the Server computed,
    // so the reader's time zone is applied once, by the party that owns the
    // calendar, and both halves agree on where a day begins.
    let request = serde_json::json!({
        "project_ids": memory.project_ids,
        "resources": memory.resources,
        "day_bounds": memory.day_bounds,
        "recency_starts": memory.recency_starts,
        "generated_at": memory.generated_at,
    });
    let response = client()
        .call(DaemonIpcRequest::new(
            "dashboard_retrieval_statistics",
            request,
        ))
        .map_err(|error| error.to_string())?;
    let retrieval: RetrievalStatistics =
        response.into_payload().map_err(|error| error.to_string())?;
    Ok(DashboardSnapshot {
        period,
        memory,
        retrieval,
        stale,
        demo: false,
    })
}

/// One period of a Dev Instance's sample, as `dev/seed-dashboard.py` writes it.
#[derive(Deserialize)]
struct FixtureSnapshot {
    /// Absent for an organization-wide sample.
    project_id: Option<String>,
    period: u32,
    memory: MemoryStatistics,
    retrieval: RetrievalStatistics,
}

/// The sample a Dev Instance keeps beside its daemon root, which is the file
/// macOS reads in one too.
///
/// It is read only when the daemon root is the one a developer pointed this
/// client at — the installed app never sets `CLUMSIES_DAEMON_ROOT` — and only
/// when the file is there and holds this Project and period. Anything else is
/// an ordinary read of the Server and the engine.
fn demo_snapshot(project_id: &str, period: Period) -> Option<DashboardSnapshot> {
    let root = std::env::var_os("CLUMSIES_DAEMON_ROOT")?;
    let mut path = std::path::PathBuf::from(root);
    path.pop();
    let text = std::fs::read_to_string(path.join("fixtures").join("dashboard.json")).ok()?;
    let mut fixture: serde_json::Value = serde_json::from_str(&text).ok()?;
    // The file is the macOS client's own spelling — `dev/seed-dashboard.py`
    // writes camelCase keys and `TimeInterval` seconds for it — so the names and
    // the numbers are translated once here rather than kept in a second set of
    // types.
    lowercase_keys(&mut fixture);
    whole_seconds(&mut fixture);
    let samples: Vec<FixtureSnapshot> = serde_json::from_value(fixture).ok()?;
    let sample = samples.into_iter().find(|sample| {
        sample.period == period.days() && sample.project_id.as_deref() == Some(project_id)
    })?;
    crate::logging::info(&format!(
        "read the Dashboard's sample for {} days",
        period.days()
    ));
    Some(DashboardSnapshot {
        period,
        memory: sample.memory,
        retrieval: sample.retrieval,
        stale: false,
        demo: true,
    })
}

/// Rewrites every key of a JSON tree from `camelCase` to `snake_case`, which is
/// the spelling this client's mirrors are written in.
fn lowercase_keys(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(fields) => {
            let translated: Vec<(String, serde_json::Value)> = std::mem::take(fields)
                .into_iter()
                .map(|(key, mut value)| {
                    lowercase_keys(&mut value);
                    (snake_case(&key), value)
                })
                .collect();
            fields.extend(translated);
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(lowercase_keys),
        _ => {}
    }
}

fn snake_case(key: &str) -> String {
    let mut spelled = String::with_capacity(key.len() + 4);
    for character in key.chars() {
        if character.is_ascii_uppercase() {
            spelled.push('_');
            spelled.push(character.to_ascii_lowercase());
        } else {
            spelled.push(character);
        }
    }
    spelled
}

/// Rewrites an instant written as a double as the whole seconds this client
/// counts in.
///
/// The sample's times are `TimeInterval` — a double, which is what the macOS
/// client reads — while this client's mirrors are seconds. A number that large
/// can only be an instant: every rate a reader is shown is a share of one, and
/// every count is already a whole number in the file.
fn whole_seconds(value: &mut serde_json::Value) {
    /// Below this a number is a share, a count or a length, never an instant.
    const INSTANT_FLOOR: f64 = 1e6;
    match value {
        serde_json::Value::Number(number) => {
            let Some(seconds) = number.as_f64() else {
                return;
            };
            if number.as_i64().is_none() && seconds.abs() >= INSTANT_FLOOR && seconds.abs() < 9e15 {
                *number = serde_json::Number::from(seconds.trunc() as i64);
            }
        }
        serde_json::Value::Object(fields) => fields.values_mut().for_each(whole_seconds),
        serde_json::Value::Array(items) => items.iter_mut().for_each(whole_seconds),
        _ => {}
    }
}

/// The two query parameters the Server requires. The time zone carries a slash,
/// so it is encoded rather than pasted into the path.
fn statistics_path(project_id: &str, period: Period, zone: &str) -> String {
    format!(
        "/api/v1/projects/{project_id}/memory-statistics?days={}&time_zone={}",
        period.days(),
        encode(zone)
    )
}

/// Percent-encodes everything a query value may not carry literally.
fn encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '~') {
            encoded.push(character);
            continue;
        }
        let mut buffer = [0_u8; 4];
        for byte in character.encode_utf8(&mut buffer).as_bytes() {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// One request through the daemon, which is the only party holding a session.
fn server(
    method: &str,
    path: &str,
    headers: BTreeMap<String, String>,
    body: Option<String>,
) -> Result<DaemonServerResponse, String> {
    client()
        .server_request(DaemonServerRequest {
            method: method.to_owned(),
            path: path.to_owned(),
            headers,
            body,
        })
        .map_err(|error| error.to_string())
}

/// A refusal carries the Server's own sentence when there is one, and its
/// status when there is not.
fn server_error(response: &DaemonServerResponse) -> String {
    serde_json::from_str::<ErrorEnvelope>(&response.body)
        .map(|envelope| {
            format!(
                "the Server answered HTTP {}: {} (request {})",
                response.status, envelope.error.message, envelope.error.request_id
            )
        })
        .unwrap_or_else(|_| format!("the Server answered HTTP {}", response.status))
}

fn client() -> DaemonIpcClient {
    DaemonIpcClient::new(DAEMON_SERVICE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_expired_sessions_require_sign_in() {
        for error in [
            "invalid_config: access_token is required",
            "server_request_failed: Server request failed with status 401: Server sign-in is required (request req_test)",
            "server_request_failed: Server request failed with status 401: Session expired (request req_test)",
        ] {
            assert!(missing_session(error), "{error}");
        }
    }

    #[test]
    fn unauthorized_server_responses_require_sign_in_with_or_without_an_envelope() {
        for (body, expected) in [
            (
                r#"{"error":{"code":"unauthorized","message":"Session expired","request_id":"req_test","details":{"private":"not displayed"}}}"#,
                "the Server answered HTTP 401: Session expired (request req_test)",
            ),
            ("Unauthorized", "the Server answered HTTP 401"),
        ] {
            let response = DaemonServerResponse {
                status: 401,
                headers: BTreeMap::new(),
                body: body.to_owned(),
            };
            let error = server_error(&response);
            assert_eq!(error, expected);
            assert!(missing_session(&error));
        }
    }

    #[test]
    fn transport_and_permission_failures_do_not_end_a_session() {
        for error in [
            "daemon IPC error: connection refused",
            "server_request_failed: HTTP connection failed (request req_401)",
            "server_request_failed: Server request failed with status 403: Forbidden",
            "server_request_failed: Server request failed with status 500: Unavailable",
        ] {
            assert!(!missing_session(error), "{error}");
        }
    }

    #[test]
    fn memory_entries_preserve_explicit_directory_metadata() {
        let resources: Vec<DaemonProjectCheckoutResource> =
            serde_json::from_value(serde_json::json!([
                { "resource_id": "dir", "scope": "project", "resource_kind": "memory",
                  "project_id": "p", "path": "notes", "content_hash": "empty",
                  "content": { "is_directory": true, "content": "" } },
                { "resource_id": "file", "scope": "project", "resource_kind": "memory",
                  "project_id": "p", "path": "notes/readme.md", "content_hash": "body",
                  "content": { "content": "Keep this file" } },
                { "resource_id": "empty", "scope": "org", "resource_kind": "memory",
                  "project_id": null, "path": "empty", "content_hash": "empty",
                  "content": { "is_directory": true, "content": "" } }
            ]))
            .unwrap();
        let directories = resources
            .iter()
            .filter(|r| r.content.is_directory)
            .cloned()
            .collect();
        assert!(
            memory_entries(directories)
                .iter()
                .all(|entry| entry.is_directory)
        );
        let documents: Vec<_> = memory_entries(resources)
            .into_iter()
            .filter(|entry| !entry.is_directory)
            .collect();
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].resource_id, "file");
        assert_eq!(documents[0].path, "notes/readme.md");
        assert_eq!(documents[0].content, "Keep this file");
    }

    #[test]
    fn statistics_are_asked_for_in_the_readers_zone() {
        assert_eq!(
            statistics_path("proj_1", Period::Month, "Asia/Shanghai"),
            "/api/v1/projects/proj_1/memory-statistics?days=30&time_zone=Asia%2FShanghai"
        );
        // A zone PostgreSQL would accept but a path would not: the plus sign.
        assert_eq!(encode("Etc/GMT+8"), "Etc%2FGMT%2B8");
    }

    #[test]
    fn the_engines_retrieval_answer_keeps_its_own_field_names() {
        // The shape `dashboard_retrieval_statistics` answers with, from
        // clumsiesd::dashboard. Pinned here because the daemon does not export
        // the nested types this client reads.
        let retrieval: RetrievalStatistics = serde_json::from_value(serde_json::json!({
            "retrievals": 12,
            "recalled_count": 5,
            "coverage": 0.25,
            "days": [{ "date": 1_790_452_800, "returned": 3, "empty": 1, "failed": 0 }],
            "directories": [{ "id": "knowledge", "label": "knowledge", "value": 4, "total": 9 }],
            "top_resources": [{ "id": "mem_1", "label": "Architecture", "value": 7 }],
            "recency": [
                { "id": "0", "label": "Last 7 days", "value": 2 },
                { "id": "1", "label": "8–30 days", "value": 1 },
                { "id": "2", "label": "31–90 days", "value": 1 },
                { "id": "3", "label": "Not observed", "value": 1 }
            ],
            "history_start": 1_790_000_000,
            "retention_per_project": 500,
            "delta": {
                "with_state": 4,
                "days": [{ "date": 1_790_452_800, "added": 2, "replaced": 1, "reused": 7, "unknown": 1 }]
            },
            "agent_usage": {
                "days": [{ "date": 1_790_452_800, "with_memory": 3, "without_memory": 1 }],
                "unreadable_sessions": 1,
                "unsupported_sessions": 2
            }
        }))
        .unwrap();
        assert_eq!(retrieval.retrievals, 12);
        assert_eq!(retrieval.directories[0].total, Some(9));
        assert_eq!(retrieval.top_resources[0].total, None);
        let delta = retrieval.delta.unwrap();
        assert_eq!(delta.total(), 10);
        assert_eq!(delta.reuse_rate(), Some(0.7));
        assert_eq!(delta.unknown(), 1);
        let usage = retrieval.agent_usage.unwrap();
        assert_eq!(usage.usage_rate(), Some(0.75));
        assert_eq!(usage.excluded_sessions(), 3);
    }

    #[test]
    fn an_engine_without_delta_telemetry_still_answers() {
        // An older daemon sends neither field, and the panels say so rather
        // than failing the whole read.
        let retrieval: RetrievalStatistics = serde_json::from_value(serde_json::json!({
            "retrievals": 0,
            "recalled_count": 0,
            "coverage": 0.0,
            "days": [],
            "directories": [],
            "top_resources": [],
            "recency": [],
            "history_start": null,
            "retention_per_project": 500
        }))
        .unwrap();
        assert!(retrieval.delta.is_none());
        assert!(retrieval.agent_usage.is_none());
    }

    #[test]
    fn a_snapshot_reads_its_own_inventory_and_names_its_documents() {
        let snapshot = DashboardSnapshot {
            period: Period::Week,
            stale: false,
            demo: false,
            memory: MemoryStatistics {
                generated_at: 1_790_500_000,
                day_bounds: vec![1_790_452_800, 1_790_539_200],
                recency_starts: vec![1_790_452_800, 1_790_452_800, 1_790_452_800],
                project_ids: vec!["proj_1".to_owned()],
                resources: vec![StatisticsResource {
                    id: "mem_1".to_owned(),
                    title: "Architecture".to_owned(),
                    path: "knowledge/architecture.md".to_owned(),
                }],
                memory_count: 1,
                added_count: 1,
                updated_count: 0,
                days: vec![
                    InventoryDay {
                        date: 1_790_452_800,
                        memory_count: None,
                    },
                    InventoryDay {
                        date: 1_790_539_200,
                        memory_count: Some(4),
                    },
                ],
                open_drafts: 1,
                submitted_drafts: 2,
            },
            retrieval: RetrievalStatistics {
                retrievals: 3,
                recalled_count: 1,
                coverage: 1.0,
                days: vec![RetrievalDay {
                    date: 1_790_539_200,
                    returned: 3,
                    empty: 0,
                    failed: 0,
                }],
                directories: Vec::new(),
                top_resources: Vec::new(),
                history_start: Some(1_790_452_800),
                retention_per_project: 500,
                delta: None,
                agent_usage: None,
            },
        };
        // A day the Server could not count is not a day of nothing: it is
        // absent from the peak rather than dragging it down.
        assert_eq!(snapshot.peak_inventory(), 4);
        assert_eq!(
            snapshot.path_for("mem_1"),
            Some("knowledge/architecture.md")
        );
        assert_eq!(snapshot.path_for("mem_2"), None);
    }

    #[test]
    fn the_samples_keys_are_read_in_this_clients_spelling() {
        // A slice of the file `dev/seed-dashboard.py` writes, in the macOS
        // client's camelCase, as it reaches the mirrors above.
        let mut fixture = serde_json::json!([{
            "projectId": "prj_1",
            "period": 7,
            "memory": {
                "generatedAt": 1_790_500_000.123,
                "dayBounds": [1_790_452_800.0, 1_790_539_200.0],
                "recencyStarts": [1_790_452_800, 1_790_452_800, 1_790_452_800],
                "projectIds": ["prj_1"],
                "resources": [{ "id": "mem_1", "title": "Architecture", "path": "knowledge/a.md" }],
                "memoryCount": 1,
                "addedCount": 1,
                "updatedCount": 0,
                "deletedCount": 0,
                "days": [{ "date": 1_790_452_800, "memoryCount": 3 }],
                "changeBuckets": [],
                "openDrafts": 9,
                "submittedDrafts": 5
            },
            "retrieval": {
                "retrievals": 12,
                "recalledCount": 1,
                "coverage": 1.0,
                "days": [{ "date": 1_790_452_800, "returned": 12, "empty": 3, "failed": 1 }],
                "directories": [{ "id": "knowledge/", "label": "knowledge/", "value": 1, "total": 1 }],
                "topResources": [{ "id": "mem_1", "label": "Architecture", "value": 12 }],
                "recency": [],
                "historyStart": 1_790_452_800,
                "retentionPerProject": 500,
                "delta": { "withState": 6, "days": [] },
                "agentUsage": { "days": [], "unreadableSessions": 0, "unsupportedSessions": 0 }
            }
        }]);
        lowercase_keys(&mut fixture);
        whole_seconds(&mut fixture);
        let samples: Vec<FixtureSnapshot> = serde_json::from_value(fixture).unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].period, 7);
        // An instant written as a double is read as the whole second it names,
        // and a rate is left a rate.
        assert_eq!(samples[0].memory.generated_at, 1_790_500_000);
        assert_eq!(samples[0].memory.day_bounds[0], 1_790_452_800);
        assert_eq!(samples[0].retrieval.coverage, 1.0);
        assert_eq!(samples[0].memory.memory_count, 1);
        assert_eq!(samples[0].memory.open_drafts, 9);
        assert_eq!(samples[0].memory.days[0].memory_count, Some(3));
        assert_eq!(samples[0].retrieval.top_resources[0].value, 12);
        assert_eq!(samples[0].retrieval.days[0].failed, 1);
        assert!(samples[0].retrieval.delta.is_some());
        assert_eq!(snake_case("projectId"), "project_id");
        assert_eq!(snake_case("dayBounds"), "day_bounds");
    }
}
