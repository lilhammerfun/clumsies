//! Everything the client reads from and writes to the local engine.
//!
//! The daemon owns the session and the Project's local state; the client asks
//! it and never reaches the Server itself. Every function here returns a
//! message rather than panicking, because a signed-out or unreachable engine is
//! a state the screens draw.

use std::collections::BTreeMap;
use std::thread::sleep;
use std::time::{Duration, Instant};

use clumsiesd::{
    DaemonContentDraftUpdate, DaemonDiscardDraftOperation, DaemonDraftContent, DaemonDraftDetail,
    DaemonDraftListQuery, DaemonDraftOperation, DaemonDraftOperationRequest,
    DaemonDraftOperationResponse, DaemonDraftOperationSource, DaemonDraftResourceKind,
    DaemonDraftScope, DaemonDraftSummary, DaemonHealth, DaemonIpcClient, DaemonIpcRequest,
    DaemonLocalDraftStatus, DaemonProjectCheckoutRequest, DaemonProjectStorageAvailability,
    DaemonProjectStorageRequest, DaemonProjectSyncRetryRequest, DaemonRetryResponse,
    DaemonServerRequest, DaemonServerResponse, DaemonUpdateDraftOperation,
    DraftOperationSyncStatus, ErrorEnvelope, SyncRetryChannel,
};
use serde::Deserialize;

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
#[derive(Clone, Deserialize)]
pub struct Project {
    pub project_id: String,
    pub name: String,
}

#[derive(Deserialize)]
struct ProjectPage {
    items: Vec<Project>,
}

/// One Memory document in the Project's current Effective Memory. The resource
/// identity is what a draft operation updates, so it travels with the text.
#[derive(Clone)]
pub struct MemoryDocument {
    pub resource_id: String,
    pub path: String,
    /// What the Project publishes today.
    pub content: String,
    /// What an open draft proposes instead. The editor opens this, because it
    /// is what the reader last wrote, and the diff measures against
    /// [Self::content], because that is what a reviewer would see.
    pub draft_content: Option<String>,
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

/// Every Memory document the Project currently resolves to, in path order.
/// The daemon serves these from its own checkout; a Project that has not
/// synced yet reports that instead of an empty list.
pub fn checkout(project_id: &str) -> Result<Checkout, String> {
    let checkout = client()
        .project_checkout(DaemonProjectCheckoutRequest {
            project_id: project_id.to_owned(),
        })
        .map_err(|error| error.to_string())?;
    let mut documents: Vec<MemoryDocument> = checkout
        .resources
        .into_iter()
        .map(|resource| MemoryDocument {
            resource_id: resource.resource_id,
            path: resource.path,
            content: resource.content.content,
            draft_content: None,
        })
        .collect();
    documents.sort_by(|left, right| left.path.cmp(&right.path));
    // A document with a proposal is opened as the proposal has it, which is
    // what the macOS client's catalog does when it builds a Project's list.
    for draft in drafts(project_id)? {
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
        }
    }
    Ok(Checkout {
        project_id: checkout.project_id,
        commit_id: checkout.commit_id,
        documents,
    })
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

/// Whether a daemon refusal means the daemon holds no Server session.
///
/// There is no flag to ask for: the daemon refuses every Server request while
/// it has no session, and that refusal is what the sign-in form is for.
pub fn missing_session(error: &str) -> bool {
    error.contains("access_token is required")
}

/// The Server the daemon is configured for, which is what the sign-in form
/// starts from.
pub fn configured_server_url() -> Option<String> {
    match client().health() {
        Ok(health) => Some(health.server_url),
        Err(_) => None,
    }
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
    pub resource_id: String,
    pub content: String,
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

/// Throws a proposal away, which is what a reader does with an edit they no
/// longer want. The published document is untouched.
pub fn discard_draft(
    draft_id: &str,
    resource_id: &str,
) -> Result<DaemonDraftOperationResponse, String> {
    let request = DaemonDraftOperationRequest {
        draft_id: Some(draft_id.to_owned()),
        base_commit_id: None,
        project_id: String::new(),
        scope: DaemonDraftScope::Project,
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

pub fn store_document(edit: &DocumentEdit) -> Result<DaemonDraftOperationResponse, String> {
    let request = DaemonDraftOperationRequest {
        draft_id: edit.draft_id.clone(),
        base_commit_id: edit.base_commit_id.clone(),
        project_id: edit.project_id.clone(),
        scope: DaemonDraftScope::Project,
        resource: DaemonDraftResourceKind::Memory,
        op: DaemonDraftOperation {
            create: None,
            update: Some(DaemonUpdateDraftOperation::Content(
                DaemonContentDraftUpdate {
                    id: edit.resource_id.clone(),
                    content: DaemonDraftContent {
                        org_source: None,
                        description: None,
                        content: edit.content.clone(),
                    },
                    description: None,
                },
            )),
            rename: None,
            delete: None,
            discard: None,
        },
        source: Some(DaemonDraftOperationSource::Desktop),
    };
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

/// One edit through to a Review: the text is stored when the editor still
/// holds something the engine has not accepted, the daemon uploads the draft,
/// and the Server turns it into a Review.
///
/// The three steps are one function because the last two are only meaningful on
/// the result of the first: the Server identifies a draft by the identity the
/// upload assigns, and a Review cannot name a draft the Server has not seen.
pub fn submit_document_review(
    edit: &DocumentEdit,
    store: bool,
    title: &str,
    description: &str,
) -> Result<Review, String> {
    let draft_id = if store {
        store_document(edit)?.draft_id
    } else {
        edit.draft_id
            .clone()
            .ok_or_else(|| "this document has no draft to review yet".to_owned())?
    };
    let draft = wait_for_upload(&draft_id)?;
    request_review(&draft, title, description)
}

/// The text a draft proposes for its document, taken from the last full
/// content update it holds. An update always carries the whole document, which
/// is what this client stores.
fn proposed_text(detail: &DaemonDraftDetail) -> Option<String> {
    detail.operations.iter().rev().find_map(|operation| {
        operation
            .operation
            .update
            .as_ref()
            .and_then(|update| update.content())
            .map(|content| content.content.clone())
    })
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
/// Asks the daemon to sync a Project's drafts now instead of on its next tick.
pub fn sync_now(project_id: &str) -> Result<(), String> {
    nudge_drafts(&client(), project_id)
}

fn nudge_drafts(client: &DaemonIpcClient, project_id: &str) -> Result<(), String> {
    let payload = serde_json::to_value(DaemonProjectSyncRetryRequest {
        project_id: project_id.to_owned(),
        channel: SyncRetryChannel::Drafts,
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
pub fn request_review(
    draft: &DaemonDraftSummary,
    title: &str,
    description: &str,
) -> Result<Review, String> {
    let Some(server_draft_id) = draft.server_draft_id.clone() else {
        return Err("the engine has not uploaded this draft yet".to_owned());
    };
    let mut headers = BTreeMap::new();
    headers.insert(
        "If-Match".to_owned(),
        format!(
            "\"{}\"",
            draft.current_commit_id.as_deref().unwrap_or("ref-none")
        ),
    );
    headers.insert("content-type".to_owned(), "application/json".to_owned());
    let body = serde_json::json!({
        "drafts": [{
            "draft_id": server_draft_id,
            "expected_draft_version": draft.server_version,
        }],
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
        .map(|envelope| envelope.error.message)
        .unwrap_or_else(|_| format!("the Server answered HTTP {}", response.status))
}

fn client() -> DaemonIpcClient {
    DaemonIpcClient::new(DAEMON_SERVICE)
}
