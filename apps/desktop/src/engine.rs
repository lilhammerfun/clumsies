//! Everything the client reads from the local engine.
//!
//! The daemon owns the session and the Project's local state; the client asks
//! it and never reaches the Server itself. Every function here returns a
//! message rather than panicking, because a signed-out or unreachable engine is
//! a state the screens draw.

use std::collections::BTreeMap;

use clumsiesd::{DaemonHealth, DaemonIpcClient, DaemonProjectCheckoutRequest, DaemonServerRequest};
use serde::Deserialize;

/// The service name the daemon registers. The client resolves it to the local
/// endpoint by the daemon's own rule, so both halves agree on where to talk.
const DAEMON_SERVICE: &str = "ai.clumsies.daemon";

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

/// One Memory document in the Project's current Effective Memory.
pub struct MemoryDocument {
    pub path: String,
    pub content: String,
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
    let response = client()
        .server_request(DaemonServerRequest {
            method: "GET".to_owned(),
            path: "/api/v1/projects".to_owned(),
            headers: BTreeMap::new(),
            body: None,
        })
        .map_err(|error| error.to_string())?;
    if response.status != 200 {
        return Err(format!("the Server answered HTTP {}", response.status));
    }
    serde_json::from_str::<ProjectPage>(&response.body)
        .map(|page| page.items)
        .map_err(|error| format!("unreadable Project list: {error}"))
}

/// Every Memory document the Project currently resolves to, in path order.
/// The daemon serves these from its own checkout; a Project that has not
/// synced yet reports that instead of an empty list.
pub fn memory_documents(project_id: &str) -> Result<Vec<MemoryDocument>, String> {
    let checkout = client()
        .project_checkout(DaemonProjectCheckoutRequest {
            project_id: project_id.to_owned(),
        })
        .map_err(|error| error.to_string())?;
    let mut documents: Vec<MemoryDocument> = checkout
        .resources
        .into_iter()
        .map(|resource| MemoryDocument {
            path: resource.path,
            content: resource.content.content,
        })
        .collect();
    documents.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(documents)
}

fn client() -> DaemonIpcClient {
    DaemonIpcClient::new(DAEMON_SERVICE)
}
