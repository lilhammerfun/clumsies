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
    DaemonContentDraftUpdate, DaemonDraftContent, DaemonDraftDetail, DaemonDraftListQuery,
    DaemonDraftOperation, DaemonDraftOperationRequest, DaemonDraftOperationResponse,
    DaemonDraftOperationSource, DaemonDraftResourceKind, DaemonDraftScope, DaemonDraftSummary,
    DaemonHealth, DaemonIpcClient, DaemonIpcRequest, DaemonLocalDraftStatus,
    DaemonProjectCheckoutRequest, DaemonProjectSyncRetryRequest, DaemonRetryResponse,
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

/// The Review the Server created for a draft. Only what this client shows is
/// modelled; the Server's answer carries much more for a Reviews screen.
#[derive(Deserialize)]
pub struct Review {
    pub review_id: String,
    pub title: String,
}

#[derive(Deserialize)]
struct ReviewDetail {
    review: Review,
}

pub fn engine_status() -> EngineStatus {
    match client().health() {
        Ok(health) => EngineStatus::Connected(health),
        Err(error) => EngineStatus::Unreachable(error.to_string()),
    }
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
    let detail: ReviewDetail = serde_json::from_str(&response.body)
        .map_err(|error| format!("unreadable Review: {error}"))?;
    Ok(detail.review)
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
