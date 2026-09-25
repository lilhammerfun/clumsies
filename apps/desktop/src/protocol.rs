//! The client side of the daemon's local wire protocol.
//!
//! The daemon serves a Unix socket (a named pipe on Windows, once that exists)
//! with one request per connection, framed as a 4-byte big-endian length
//! followed by JSON. The shapes below are the daemon's `types.rs` contract;
//! only the parts this client reads are mirrored here, on purpose: depending on
//! the daemon crate would link a 90 MB static ONNX Runtime into a client that
//! never runs inference. The longer-term home for both halves is a protocol
//! crate they can share.

use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// A local socket answers immediately or not at all; this only bounds a daemon
/// that accepted the connection and then wedged.
const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

#[derive(Serialize)]
struct Request<'a> {
    method: &'a str,
    payload: serde_json::Value,
}

#[derive(Deserialize)]
struct Response {
    ok: bool,
    payload: serde_json::Value,
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct ApiError {
    message: String,
}

/// What the client knows about the engine. Everything the Memory screen will
/// show arrives the same way, so this is the whole seam today.
#[derive(Clone)]
pub enum EngineStatus {
    Connected(Health),
    Unreachable(String),
}

#[derive(Clone, Deserialize)]
pub struct Health {
    pub daemon_version: String,
    pub daemon_installation_id: String,
    pub server_url: String,
    pub project_id: Option<String>,
    pub local_db: LocalDb,
}

#[derive(Clone, Deserialize)]
pub struct LocalDb {
    /// Mirrored for completeness: the daemon reports it, and a future screen
    /// that shows storage locations reads it from here.
    #[allow(dead_code)]
    pub path: String,
    pub schema_version: i64,
}

/// Asks the engine how it is. Never fails: an engine that is not running is a
/// state the client draws, not an error it propagates.
pub fn health() -> EngineStatus {
    match call("health", serde_json::json!({})) {
        Ok(payload) => match serde_json::from_value::<Health>(payload) {
            Ok(health) => EngineStatus::Connected(health),
            Err(error) => EngineStatus::Unreachable(format!("unreadable reply: {error}")),
        },
        Err(error) => EngineStatus::Unreachable(error),
    }
}

fn call(method: &str, payload: serde_json::Value) -> Result<serde_json::Value, String> {
    let path = socket_path();
    let mut stream =
        UnixStream::connect(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    stream
        .set_read_timeout(Some(TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(TIMEOUT)))
        .map_err(|error| format!("could not configure the connection: {error}"))?;

    let request = serde_json::to_vec(&Request { method, payload })
        .map_err(|error| format!("could not encode the request: {error}"))?;
    let length =
        u32::try_from(request.len()).map_err(|_| "the request is too large to frame".to_owned())?;
    stream
        .write_all(&length.to_be_bytes())
        .and_then(|()| stream.write_all(&request))
        .and_then(|()| stream.flush())
        .map_err(|error| format!("could not send the request: {error}"))?;

    let mut header = [0u8; 4];
    stream
        .read_exact(&mut header)
        .map_err(|error| format!("could not read the reply: {error}"))?;
    let length = u32::from_be_bytes(header) as usize;
    if length > MAX_FRAME_BYTES {
        return Err(format!("the reply claims {length} bytes, above the limit"));
    }
    let mut body = vec![0u8; length];
    stream
        .read_exact(&mut body)
        .map_err(|error| format!("could not read the reply body: {error}"))?;

    let response: Response = serde_json::from_slice(&body)
        .map_err(|error| format!("could not decode the reply: {error}"))?;
    if !response.ok {
        return Err(response
            .error
            .map(|error| error.message)
            .unwrap_or_else(|| "the engine reported a failure without details".to_owned()));
    }
    Ok(response.payload)
}

/// The daemon's own rule for where it listens: an explicit socket, then an
/// explicit root, then the XDG data directory.
fn socket_path() -> PathBuf {
    if let Some(value) = std::env::var_os("CLUMSIES_DAEMON_SOCKET") {
        return PathBuf::from(value);
    }
    if let Some(value) = std::env::var_os("CLUMSIES_DAEMON_ROOT") {
        return PathBuf::from(value).join("daemon.sock");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    data.join("ai.clumsies").join("daemon.sock")
}
