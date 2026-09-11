//! MCP JSON-RPC stdio adapter for the short-lived daemon proxy mode.

use std::io::{self, BufRead, Write};

use serde_json::{Map, Value, json};

use super::AgentRuntimeBackend;
use super::mcp_contract::{
    DEFAULT_GUIDELINES_PATH, decode_tool_call_params, parse_tool_call,
    tool_definitions_with_guidelines,
};

const JSONRPC_VERSION: &str = "2.0";
pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const MAX_MESSAGE_SIZE: usize = 1024 * 1024;

pub fn server_instructions(guidelines_path: &str) -> String {
    format!(
        "Call memory with op: {{ activate: {{ query: \"...\" }} }} once at the start of every substantive user task. It returns ranked memory fragments ready for the current reasoning context. Pass its next_state only while the earlier fragments remain in the model context; omit state after context compaction or when starting fresh. Use memory with op: {{ load: {{ ids: [...] }} }} to read complete resources by known id or path (including project memory conventions at {guidelines_path}). Use memory with op: {{ store: ... }} only when the user explicitly asks for memory maintenance; adhere to project memory update rules before storing drafts. Store creates a proposal carried by the bound Project and changes only that Project's pre-merge Effective Memory overlay; it cannot decide a Review, merge, or publish Organization authority."
    )
}

#[derive(Clone, Copy)]
enum ErrorCode {
    Parse = -32700,
    InvalidRequest = -32600,
    MethodNotFound = -32601,
    ServerNotInitialized = -32002,
}

/// Stateful MCP protocol processor. The backend is an IPC-only boundary; this
/// type never creates daemon state or writes protocol diagnostics to stdout.
pub struct McpServer<B> {
    backend: B,
    project_id: String,
    guidelines_path: String,
    version: String,
    initialize_seen: bool,
    initialized: bool,
}

impl<B> McpServer<B>
where
    B: AgentRuntimeBackend,
{
    pub fn new(backend: B, project_id: impl Into<String>, version: impl Into<String>) -> Self {
        let guidelines_path = backend
            .guidelines_path()
            .ok()
            .flatten()
            .unwrap_or_else(|| DEFAULT_GUIDELINES_PATH.to_owned());
        Self {
            backend,
            project_id: project_id.into(),
            guidelines_path,
            version: version.into(),
            initialize_seen: false,
            initialized: false,
        }
    }

    pub fn with_guidelines_path(mut self, path: impl Into<String>) -> Self {
        self.guidelines_path = path.into();
        self
    }

    /// Processes one newline-delimited JSON-RPC message. Notifications return
    /// no value; callers serialize returned values exclusively to stdout.
    pub fn process_line(&mut self, line: &[u8]) -> Option<Value> {
        if line.len() > MAX_MESSAGE_SIZE {
            return Some(error_response(
                Value::Null,
                ErrorCode::Parse,
                "message exceeds MAX_MESSAGE_SIZE",
            ));
        }
        let line = trim_ascii_whitespace(line);
        if line.is_empty() {
            return None;
        }
        let message = match serde_json::from_slice::<Value>(line) {
            Ok(message) => message,
            Err(_) => {
                return Some(error_response(
                    Value::Null,
                    ErrorCode::Parse,
                    "Invalid JSON",
                ));
            }
        };
        self.process_message(message)
    }

    pub fn serve<R, W>(&mut self, input: &mut R, output: &mut W) -> io::Result<()>
    where
        R: BufRead,
        W: Write,
    {
        loop {
            match read_bounded_line(input, MAX_MESSAGE_SIZE)? {
                BoundedLine::Eof => return Ok(()),
                BoundedLine::TooLong => {
                    write_response(
                        output,
                        &error_response(
                            Value::Null,
                            ErrorCode::Parse,
                            "message exceeds MAX_MESSAGE_SIZE",
                        ),
                    )?;
                }
                BoundedLine::Line(line) => {
                    if let Some(response) = self.process_line(&line) {
                        write_response(output, &response)?;
                    }
                }
            }
        }
    }

    fn process_message(&mut self, message: Value) -> Option<Value> {
        let object = match message {
            Value::Object(object) => object,
            Value::Array(_) => {
                return Some(error_response(
                    Value::Null,
                    ErrorCode::InvalidRequest,
                    "Batch requests are not supported",
                ));
            }
            _ => {
                return Some(error_response(
                    Value::Null,
                    ErrorCode::InvalidRequest,
                    "Expected JSON object",
                ));
            }
        };
        let method = match object.get("method") {
            Some(Value::String(method)) => method.clone(),
            Some(_) => {
                return Some(error_response(
                    request_id(&object),
                    ErrorCode::InvalidRequest,
                    "Method must be a string",
                ));
            }
            None => {
                return Some(error_response(
                    request_id(&object),
                    ErrorCode::InvalidRequest,
                    "Missing method",
                ));
            }
        };
        let Some(id) = object.get("id").cloned() else {
            self.handle_notification(&method);
            return None;
        };
        let id = sanitized_id(id);
        let params = object.get("params").cloned().unwrap_or(Value::Null);

        match method.as_str() {
            "initialize" => {
                self.initialize_seen = true;
                Some(success_response(
                    id,
                    json!({
                        "protocolVersion": PROTOCOL_VERSION,
                        "capabilities": {"tools": {"listChanged": false}},
                        "serverInfo": {"name": "clumsies", "version": self.version},
                        "instructions": server_instructions(&self.guidelines_path)
                    }),
                ))
            }
            "ping" => Some(success_response(id, json!({}))),
            _ if !self.initialized => Some(error_response(
                id,
                ErrorCode::ServerNotInitialized,
                "Server not initialized",
            )),
            "tools/list" => Some(success_response(
                id,
                json!({"tools": tool_definitions_with_guidelines(&self.guidelines_path)}),
            )),
            "tools/call" => Some(success_response(id, self.call_tool(params))),
            _ => Some(error_response(
                id,
                ErrorCode::MethodNotFound,
                "Unknown method",
            )),
        }
    }

    fn handle_notification(&mut self, method: &str) {
        if method == "notifications/initialized" && self.initialize_seen {
            self.initialized = true;
        }
    }

    fn call_tool(&self, params: Value) -> Value {
        let params = match decode_tool_call_params(params) {
            Ok(params) => params,
            Err(error) => return tool_error(error.to_string()),
        };
        let request = match parse_tool_call(&self.project_id, &params.name, params.arguments) {
            Ok(request) => request,
            Err(error) => return tool_error(error.to_string()),
        };
        match self.backend.execute(request) {
            Ok(response) if response.ok => tool_success(response.payload),
            Ok(response) => match response.error {
                Some(error) => {
                    let message = error.message.clone();
                    tool_structured_error(message, json!({"error": error}))
                }
                None => tool_error("local daemon rejected the operation without error details"),
            },
            Err(error) => {
                let error = crate::types::api_error_from_daemon_error(error);
                tool_structured_error(
                    format!("{} (request {})", error.message, error.request_id),
                    json!({"error": error}),
                )
            }
        }
    }
}

fn request_id(object: &Map<String, Value>) -> Value {
    object
        .get("id")
        .cloned()
        .map(sanitized_id)
        .unwrap_or(Value::Null)
}

fn sanitized_id(value: Value) -> Value {
    match value {
        Value::Null | Value::Number(_) | Value::String(_) => value,
        _ => Value::Null,
    }
}

fn success_response(id: Value, result: Value) -> Value {
    json!({"jsonrpc": JSONRPC_VERSION, "id": id, "result": result})
}

fn error_response(id: Value, code: ErrorCode, message: &str) -> Value {
    json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": id,
        "error": {"code": code as i32, "message": message}
    })
}

fn tool_success(structured: Value) -> Value {
    let text = serde_json::to_string(&structured).unwrap_or_else(|_| "{}".to_owned());
    json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": structured,
        "isError": false
    })
}

fn tool_error(message: impl Into<String>) -> Value {
    let message = message.into();
    tool_structured_error(message.clone(), json!({"error": message}))
}

fn tool_structured_error(message: String, structured: Value) -> Value {
    json!({
        "content": [{"type": "text", "text": message}],
        "structuredContent": structured,
        "isError": true
    })
}

fn trim_ascii_whitespace(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

enum BoundedLine {
    Eof,
    Line(Vec<u8>),
    TooLong,
}

fn read_bounded_line<R: BufRead>(input: &mut R, max: usize) -> io::Result<BoundedLine> {
    let mut line = Vec::new();
    let mut too_long = false;
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            return if line.is_empty() && !too_long {
                Ok(BoundedLine::Eof)
            } else if too_long {
                Ok(BoundedLine::TooLong)
            } else {
                Ok(BoundedLine::Line(line))
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |position| position + 1);
        if !too_long {
            if line.len().saturating_add(consumed) > max.saturating_add(1) {
                too_long = true;
                line.clear();
            } else {
                line.extend_from_slice(&available[..consumed]);
            }
        }
        input.consume(consumed);
        if newline.is_some() {
            return if too_long {
                Ok(BoundedLine::TooLong)
            } else {
                Ok(BoundedLine::Line(line))
            };
        }
    }
}

fn write_response<W: Write>(output: &mut W, response: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, response)?;
    output.write_all(b"\n")?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Cursor};
    use std::sync::Mutex;

    use crate::{ApiError, DaemonError, DaemonIpcResponse};

    use super::*;
    use crate::agent_runtime::mcp_contract::AgentRuntimeRequest;

    struct RecordingBackend {
        requests: Mutex<Vec<AgentRuntimeRequest>>,
        response: DaemonIpcResponse,
    }

    impl RecordingBackend {
        fn success(payload: Value) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                response: DaemonIpcResponse {
                    ok: true,
                    payload,
                    error: None,
                },
            }
        }
    }

    impl AgentRuntimeBackend for RecordingBackend {
        fn execute(&self, request: AgentRuntimeRequest) -> Result<DaemonIpcResponse, DaemonError> {
            self.requests.lock().unwrap().push(request);
            Ok(self.response.clone())
        }
    }

    fn initialized_server(backend: RecordingBackend) -> McpServer<RecordingBackend> {
        let mut server = McpServer::new(backend, "prj_test", "0.16.3");
        assert!(
            server
                .process_line(br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#)
                .is_some()
        );
        assert!(
            server
                .process_line(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .is_none()
        );
        server
    }

    #[test]
    fn enforces_initialize_then_initialized_before_listing_tools() {
        let backend = RecordingBackend::success(json!({}));
        let mut server = McpServer::new(backend, "prj_test", "0.16.3");
        let early = server
            .process_line(br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
            .unwrap();
        assert_eq!(early["error"]["code"], -32002);

        let initialize = server
            .process_line(br#"{"jsonrpc":"2.0","id":"init","method":"initialize"}"#)
            .unwrap();
        assert_eq!(initialize["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(initialize["result"]["serverInfo"]["version"], "0.16.3");

        server.process_line(br#"{"method":"notifications/initialized"}"#);
        let list = server
            .process_line(br#"{"id":2,"method":"tools/list","params":{}}"#)
            .unwrap();
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn typed_activate_call_reaches_backend_and_wraps_structured_content() {
        let backend = RecordingBackend::success(json!({"next_state": "opaque"}));
        let mut server = initialized_server(backend);
        let response = server
            .process_line(
                br#"{"id":3,"method":"tools/call","params":{"name":"memory","arguments":{"op":{"activate":{"query":"runtime identity"}}}}}"#,
            )
            .unwrap();

        assert_eq!(response["result"]["isError"], false);
        assert_eq!(
            response["result"]["structuredContent"]["next_state"],
            "opaque"
        );
        let requests = server.backend.requests.lock().unwrap();
        let AgentRuntimeRequest::Activate(request) = &requests[0] else {
            panic!("unexpected request variant");
        };
        assert_eq!(request.project_id, "prj_test");
        assert_eq!(request.query, "runtime identity");
    }

    #[test]
    fn invalid_tool_arguments_are_tool_errors_and_never_reach_ipc() {
        let backend = RecordingBackend::success(json!({}));
        let mut server = initialized_server(backend);
        let response = server
            .process_line(
                br#"{"id":4,"method":"tools/call","params":{"name":"memory","arguments":{"op":{"activate":{"query":"ok","prompt":"secret"}}}}}"#,
            )
            .unwrap();

        assert_eq!(response["result"]["isError"], true);
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("unknown field")
        );
        assert!(server.backend.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn preserves_daemon_structured_errors() {
        let backend = RecordingBackend {
            requests: Mutex::new(Vec::new()),
            response: DaemonIpcResponse {
                ok: false,
                payload: json!({}),
                error: Some(ApiError {
                    code: "index_preparing".to_owned(),
                    message: "Index is preparing".to_owned(),
                    request_id: "req_test".to_owned(),
                    details: json!({"completed": 3}),
                }),
            },
        };
        let mut server = initialized_server(backend);
        let response = server
            .process_line(
                br#"{"id":5,"method":"tools/call","params":{"name":"memory","arguments":{"op":{"activate":{"query":"hello"}}}}}"#,
            )
            .unwrap();

        assert_eq!(
            response["result"]["structuredContent"]["error"]["code"],
            "index_preparing"
        );
        assert_eq!(response["result"]["isError"], true);
    }

    #[test]
    fn stdio_loop_bounds_and_recovers_after_an_oversized_line() {
        let mut input = vec![b'x'; MAX_MESSAGE_SIZE + 1];
        input.extend_from_slice(b"\n{\"id\":7,\"method\":\"ping\"}\n");
        let mut input = BufReader::new(Cursor::new(input));
        let mut output = Vec::new();
        let backend = RecordingBackend::success(json!({}));
        let mut server = McpServer::new(backend, "prj_test", "0.16.3");

        server.serve(&mut input, &mut output).unwrap();

        let lines = output.split(|byte| *byte == b'\n').collect::<Vec<_>>();
        let too_large: Value = serde_json::from_slice(lines[0]).unwrap();
        let ping: Value = serde_json::from_slice(lines[1]).unwrap();
        assert_eq!(too_large["error"]["code"], -32700);
        assert_eq!(ping["result"], json!({}));
    }

    #[test]
    fn server_injects_custom_guidelines_path_into_instructions_and_tool_definition() {
        let backend = RecordingBackend::success(json!({}));
        let mut server = McpServer::new(backend, "prj_test", "0.16.3")
            .with_guidelines_path("./docs/MEMORY_RULES.md");

        let init = server
            .process_line(br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#)
            .unwrap();
        assert!(
            init["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("./docs/MEMORY_RULES.md")
        );

        server.process_line(br#"{"method":"notifications/initialized"}"#);
        let list = server
            .process_line(br#"{"id":2,"method":"tools/list","params":{}}"#)
            .unwrap();
        let tools = list["result"]["tools"].as_array().unwrap();
        let memory_tool = tools.iter().find(|t| t["name"] == "memory").unwrap();
        assert!(
            memory_tool["description"]
                .as_str()
                .unwrap()
                .contains("./docs/MEMORY_RULES.md")
        );
    }
}
