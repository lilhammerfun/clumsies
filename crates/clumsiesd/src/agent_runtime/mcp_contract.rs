//! MCP Memory tool schema and request validation for bound projects.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;

use crate::{
    ActivateMemoryRequest, DaemonCreateDraftOperation, DaemonDeleteDraftOperation,
    DaemonDiscardDraftOperation, DaemonDraftContent, DaemonDraftOperation,
    DaemonDraftOperationRequest, DaemonDraftOperationSource, DaemonDraftResourceKind,
    DaemonDraftScope, DaemonRenameDraftOperation, DaemonTextDraftUpdate, DaemonTextReplacement,
    DaemonUpdateDraftOperation, LoadMemoryRequest,
};

pub const MEMORY_TOOL_NAME: &str = "memory";

#[derive(Debug, Error, PartialEq, Eq)]
#[error("{0}")]
pub struct ContractError(String);

impl ContractError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// Typed requests accepted by the resident daemon after the Agent protocol
/// boundary has validated and normalized its own contract.
#[derive(Debug)]
pub enum AgentRuntimeRequest {
    Activate(ActivateMemoryRequest),
    Load(LoadMemoryRequest),
    Store(Box<DaemonDraftOperationRequest>),
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolCallParams {
    pub(crate) name: String,
    #[serde(default = "empty_object")]
    pub(crate) arguments: Value,
    #[serde(default)]
    pub(crate) _meta: Option<Value>,
}

fn empty_object() -> Value {
    json!({})
}

pub(crate) fn decode_tool_call_params(value: Value) -> Result<ToolCallParams, ContractError> {
    serde_json::from_value(value)
        .map_err(|error| ContractError::new(format!("invalid request: {error}")))
}

pub fn parse_tool_call(
    project_id: &str,
    name: &str,
    arguments: Value,
) -> Result<AgentRuntimeRequest, ContractError> {
    if !arguments.is_object() {
        return Err(ContractError::new(
            "invalid request: arguments must be a JSON object",
        ));
    }
    reject_null_values(&arguments)?;

    match name {
        MEMORY_TOOL_NAME => {
            let input: MemoryInput = decode_arguments(name, arguments)?;
            input.into_domain(project_id)
        }
        _ => Err(ContractError::new("Unknown tool")),
    }
}

fn decode_arguments<T>(name: &str, value: Value) -> Result<T, ContractError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value(value)
        .map_err(|error| ContractError::new(format!("invalid {name} arguments: {error}")))
}

fn reject_null_values(value: &Value) -> Result<(), ContractError> {
    match value {
        Value::Null => Err(ContractError::new("arguments must not contain null values")),
        Value::Array(values) => {
            for value in values {
                reject_null_values(value)?;
            }
            Ok(())
        }
        Value::Object(values) => {
            for value in values.values() {
                reject_null_values(value)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryInput {
    op: MemoryOperation,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MemoryOperation {
    Activate(ActivateInput),
    Load(LoadInput),
    Store(Box<StoreInput>),
}

impl MemoryInput {
    fn into_domain(self, project_id: &str) -> Result<AgentRuntimeRequest, ContractError> {
        match self.op {
            MemoryOperation::Activate(input) => input.into_domain(project_id),
            MemoryOperation::Load(input) => input.into_domain(project_id),
            MemoryOperation::Store(input) => input.into_domain(project_id),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivateInput {
    query: String,
    state: Option<String>,
}

impl ActivateInput {
    fn into_domain(self, project_id: &str) -> Result<AgentRuntimeRequest, ContractError> {
        if self.query.trim().is_empty() {
            return Err(ContractError::new("query must not be empty"));
        }
        Ok(AgentRuntimeRequest::Activate(ActivateMemoryRequest {
            project_id: project_id.to_owned(),
            query: self.query,
            state: self.state,
        }))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LoadInput {
    ids: Vec<String>,
    known_hashes: Option<BTreeMap<String, String>>,
}

impl LoadInput {
    fn into_domain(self, project_id: &str) -> Result<AgentRuntimeRequest, ContractError> {
        if self.ids.is_empty()
            || self.ids.iter().any(String::is_empty)
            || self.ids.iter().collect::<BTreeSet<_>>().len() != self.ids.len()
        {
            return Err(ContractError::new(
                "ids is required and must contain unique non-empty strings",
            ));
        }
        Ok(AgentRuntimeRequest::Load(LoadMemoryRequest {
            project_id: project_id.to_owned(),
            ids: self.ids,
            known_hashes: self.known_hashes.unwrap_or_default(),
        }))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum StoreResource {
    Memory,
}

impl StoreResource {
    fn domain(self) -> DaemonDraftResourceKind {
        DaemonDraftResourceKind::Memory
    }

    fn content(self, body: String, description: Option<String>) -> DaemonDraftContent {
        DaemonDraftContent {
            org_source: None,
            description,
            content: body,
        }
    }
}

fn default_store_resource() -> StoreResource {
    StoreResource::Memory
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreInput {
    #[serde(default = "default_store_resource")]
    resource: StoreResource,
    #[serde(default)]
    create: Option<StoreCreateInput>,
    #[serde(default)]
    update: Option<StoreUpdateInput>,
    #[serde(default)]
    rename: Option<StoreRenameInput>,
    #[serde(default)]
    delete: Option<StoreDeleteInput>,
    #[serde(default)]
    discard: Option<StoreDiscardInput>,
    #[serde(default)]
    op: Option<StoreOperation>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoreOperation {
    Create(StoreCreateInput),
    Update(StoreUpdateInput),
    Rename(StoreRenameInput),
    Delete(StoreDeleteInput),
    Discard(StoreDiscardInput),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreCreateInput {
    path: String,
    body: String,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreUpdateInput {
    id: String,
    expected_hash: String,
    replacements: Vec<TextReplacementInput>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TextReplacementInput {
    old_text: String,
    new_text: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreRenameInput {
    id: String,
    new_path: String,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreDeleteInput {
    id: String,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreDiscardInput {
    id: String,
}

impl StoreInput {
    fn into_domain(self, project_id: &str) -> Result<AgentRuntimeRequest, ContractError> {
        let op = match (
            self.op,
            self.create,
            self.update,
            self.rename,
            self.delete,
            self.discard,
        ) {
            (Some(op), None, None, None, None, None) => op,
            (None, Some(create), None, None, None, None) => StoreOperation::Create(create),
            (None, None, Some(update), None, None, None) => StoreOperation::Update(update),
            (None, None, None, Some(rename), None, None) => StoreOperation::Rename(rename),
            (None, None, None, None, Some(delete), None) => StoreOperation::Delete(delete),
            (None, None, None, None, None, Some(discard)) => StoreOperation::Discard(discard),
            _ => {
                return Err(ContractError::new(
                    "store must provide exactly one operation (create, update, rename, delete, or discard)",
                ));
            }
        };

        let resource = self.resource.domain();
        let mut operation = DaemonDraftOperation {
            create: None,
            update: None,
            rename: None,
            delete: None,
            discard: None,
        };
        match op {
            StoreOperation::Create(input) => {
                require_non_empty(&input.path, "path")?;
                require_non_empty(&input.body, "body")?;
                validate_workflow_path(self.resource, &input.path)?;
                operation.create = Some(DaemonCreateDraftOperation {
                    path: input.path,
                    content: self.resource.content(input.body, input.description.clone()),
                    description: input.description,
                });
            }
            StoreOperation::Update(input) => {
                require_non_empty(&input.id, "id")?;
                require_non_empty(&input.expected_hash, "expected_hash")?;
                if input.replacements.is_empty() {
                    return Err(ContractError::new(
                        "replacements must contain at least one text replacement",
                    ));
                }
                if input
                    .replacements
                    .iter()
                    .any(|replacement| replacement.old_text.is_empty())
                {
                    return Err(ContractError::new("replacement old_text must not be empty"));
                }
                operation.update = Some(DaemonUpdateDraftOperation::Text(DaemonTextDraftUpdate {
                    id: input.id,
                    expected_hash: input.expected_hash,
                    replacements: input
                        .replacements
                        .into_iter()
                        .map(|replacement| DaemonTextReplacement {
                            old_text: replacement.old_text,
                            new_text: replacement.new_text,
                        })
                        .collect(),
                    description: input.description,
                }));
            }
            StoreOperation::Rename(input) => {
                require_non_empty(&input.id, "id")?;
                require_non_empty(&input.new_path, "new_path")?;
                validate_workflow_path(self.resource, &input.new_path)?;
                operation.rename = Some(DaemonRenameDraftOperation {
                    id: input.id,
                    new_path: input.new_path,
                    description: input.description,
                });
            }
            StoreOperation::Delete(input) => {
                require_non_empty(&input.id, "id")?;
                operation.delete = Some(DaemonDeleteDraftOperation {
                    id: input.id,
                    description: input.description,
                });
            }
            StoreOperation::Discard(input) => {
                require_non_empty(&input.id, "id")?;
                operation.discard = Some(DaemonDiscardDraftOperation { id: input.id });
            }
        }
        Ok(AgentRuntimeRequest::Store(Box::new(
            DaemonDraftOperationRequest {
                draft_id: None,
                base_commit_id: None,
                project_id: project_id.to_owned(),
                // The bound Project carries this LocalDraft and is the only place
                // its overlay is visible before merge. Org is the proposal's
                // authority target, not a Project and not directly writable here.
                scope: DaemonDraftScope::Project,
                resource,
                op: operation,
                source: Some(DaemonDraftOperationSource::McpStore),
            },
        )))
    }
}

fn validate_workflow_path(resource: StoreResource, path: &str) -> Result<(), ContractError> {
    if false && resource == StoreResource::Memory && !path.starts_with("workflow/") {
        return Err(ContractError::new(
            "workflow paths must use the workflow/ namespace",
        ));
    }
    Ok(())
}

fn require_non_empty(value: &str, name: &str) -> Result<(), ContractError> {
    if value.is_empty() {
        return Err(ContractError::new(format!("{name} must not be empty")));
    }
    Ok(())
}

pub const DEFAULT_GUIDELINES_PATH: &str = "CLUMSIES.md";

/// Describe the user-owned maintenance guide consistently across MCP discovery surfaces.
pub(super) fn memory_guidelines_instructions(guidelines_path: &str) -> String {
    format!(
        "Memory Guidelines at {guidelines_path} define what to keep and how to organize, update, and retire knowledge in Clumsies. Before Clumsies memory maintenance, load this exact Effective Memory path with op.load; reuse a current guide already in context for the same task. Follow the user's applicable conventions and customizations. The guide is ordinary Memory, not a host file or extra write authorization. If missing, report the path and continue reads and fully specified authorized edits using the user's instructions and existing conventions; clarify only decisions that depend on the missing guide. Do not initialize or replace it automatically, or fall back from a missing custom path. For optional setup, direct the user to the App's empty Project Memory view: Set Up Guidelines creates CLUMSIES.md and starter folder READMEs as Drafts, preserving existing folders; Use Team Guidelines selects an existing resource and requires Project administrator access. The bundled template becomes Memory only after the user adopts it."
    )
}

/// Tool definitions exposed over MCP. Keep these schemas MCP-specific: daemon
/// request structs intentionally do not double as public Agent contracts.
pub fn tool_definitions() -> Vec<Value> {
    tool_definitions_with_guidelines(DEFAULT_GUIDELINES_PATH)
}

pub fn tool_definitions_with_guidelines(guidelines_path: &str) -> Vec<Value> {
    vec![memory_tool_definition(guidelines_path)]
}

/// Expose the bound project's Memory operations and their storage boundaries.
fn memory_tool_definition(guidelines_path: &str) -> Value {
    let guidelines = memory_guidelines_instructions(guidelines_path);
    let description = format!(
        "Consult Clumsies project memory before answering project questions, planning, implementing, debugging, or reviewing, even when the user does not mention memory. Follow applicable host memory policies and additionally query Clumsies, even when host memory has already been consulted. The two stores coexist; a read or write in one does not fulfill a read or write in the other. Recall decisions, conventions, procedures, and project-maintained skills, or remember, record, correct, update, or delete project knowledge when explicitly requested. Respect explicit user requests to skip Clumsies or use only another source or destination. Activate ranked fragments, load complete resources by ID or path, or store Project-owned proposal Drafts. Ordinary development and current-task reminders do not authorize Clumsies memory writes. Store never writes Organization authority; publication requires an authorized Review decision and merge. Report the saved resource and Draft status. {guidelines} Pass exactly one tagged operation in op."
    );
    json!({
        "name": MEMORY_TOOL_NAME,
        "title": "Clumsies Project Memory",
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": {
                "op": {
                    "type": "object",
                    "minProperties": 1,
                    "maxProperties": 1,
                    "properties": {
                        "activate": {
                            "type": "object",
                            "description": "Retrieve Clumsies memory relevant to the current project task or recall project decisions, conventions, procedures, and skills. Call before project analysis, planning, or editing, even without an explicit request to search memory; reuse the current task's activation while its fragments remain in context. Pass state only while fragments from the preceding activation remain in the model context.",
                            "properties": {
                                "query": {
                                    "type": "string",
                                    "minLength": 1,
                                    "description": "Natural-language representation of the current task or retrieval cue."
                                },
                                "state": {
                                    "type": "string",
                                    "description": "Opaque next_state from a preceding activation whose fragments are still present in the current model context."
                                }
                            },
                            "required": ["query"],
                            "additionalProperties": false
                        },
                        "load": {
                            "type": "object",
                            "description": "Load complete current memory resources by stable id or exact path. Use for deep reading or before editing a known resource; activate already returns directly usable fragments and does not require a follow-up load.",
                            "properties": {
                                "ids": {
                                    "type": "array",
                                    "minItems": 1,
                                    "uniqueItems": true,
                                    "items": {"type": "string", "minLength": 1},
                                    "description": "Stable resource ids or exact paths."
                                },
                                "knownHashes": {
                                    "type": "object",
                                    "description": "Optional known content hashes keyed by requested id or path. Unchanged resources omit content.",
                                    "additionalProperties": {"type": "string"}
                                }
                            },
                            "required": ["ids"],
                            "additionalProperties": false
                        },
                        "store": {
                            "type": "object",
                            "description": "Create, update, rename, delete, or discard a Clumsies Memory proposal Draft when the user explicitly requests memory maintenance, such as remembering, recording, correcting, updating, or deleting project knowledge. Ordinary development and current-task reminders do not authorize writes. Edits to selected Org references create explicit Project adaptations. Before merge its overlay affects only the bound Project's Effective Memory. Before update, load the complete resource and use its content_hash with exact text replacements; update never accepts a complete document body. A successful call means durable local persistence and queued synchronization, not an authorized Review decision, merge, or Organization authority publication. Follow the project memory conventions identified in the tool description and report the saved resource and Draft status.",
                            "properties": {
                                "resource": {
                                    "type": "string",
                                    "enum": ["memory"],
                                    "description": "Memory resource type. Defaults to 'memory'."
                                },
                                "create": {"$ref": "#/$defs/writeCreate"},
                                "update": {"$ref": "#/$defs/writeUpdate"},
                                "rename": {
                                    "type": "object",
                                    "properties": {
                                        "id": {"type": "string", "minLength": 1},
                                        "new_path": {"type": "string", "minLength": 1},
                                        "description": {"type": "string"}
                                    },
                                    "required": ["id", "new_path"],
                                    "additionalProperties": false
                                },
                                "delete": {
                                    "type": "object",
                                    "properties": {
                                        "id": {"type": "string", "minLength": 1},
                                        "description": {"type": "string"}
                                    },
                                    "required": ["id"],
                                    "additionalProperties": false
                                },
                                "discard": {
                                    "type": "object",
                                    "properties": {"id": {"type": "string", "minLength": 1}},
                                    "required": ["id"],
                                    "additionalProperties": false
                                }
                            },
                            "additionalProperties": false
                        }
                    },
                    "additionalProperties": false
                }
            },
            "required": ["op"],
            "additionalProperties": false,
            "$defs": {
                "writeCreate": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "minLength": 1},
                        "body": {"type": "string", "minLength": 1},
                        "description": {"type": "string"}
                    },
                    "required": ["path", "body"],
                    "additionalProperties": false
                },
                "writeUpdate": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "minLength": 1,
                            "description": "Stable resource_id returned by load, not a path."
                        },
                        "expected_hash": {
                            "type": "string",
                            "minLength": 1,
                            "description": "Complete-resource content_hash returned by load."
                        },
                        "replacements": {
                            "type": "array",
                            "minItems": 1,
                            "description": "Atomic, non-overlapping exact text replacements matched against the loaded resource.",
                            "items": {"$ref": "#/$defs/textReplacement"}
                        },
                        "description": {"type": "string"}
                    },
                    "required": ["id", "expected_hash", "replacements"],
                    "additionalProperties": false
                },
                "textReplacement": {
                    "type": "object",
                    "properties": {
                        "old_text": {"type": "string", "minLength": 1},
                        "new_text": {"type": "string"}
                    },
                    "required": ["old_text", "new_text"],
                    "additionalProperties": false
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_expose_only_memory() {
        let tools = tool_definitions();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], MEMORY_TOOL_NAME);
        assert!(tools[0]["inputSchema"]["properties"]["op"]["properties"]["activate"].is_object());
        assert!(tools[0]["inputSchema"]["properties"]["op"]["properties"]["load"].is_object());
        assert!(tools[0]["inputSchema"]["properties"]["op"]["properties"]["store"].is_object());
    }

    #[test]
    fn strict_contract_rejects_unknown_null_and_multiple_operations() {
        let unknown = parse_tool_call(
            "prj_test",
            MEMORY_TOOL_NAME,
            json!({"op": {"activate": {"query": "hello", "raw_payload": "secret"}}}),
        )
        .unwrap_err();
        assert!(unknown.to_string().contains("unknown field"));

        let null = parse_tool_call(
            "prj_test",
            MEMORY_TOOL_NAME,
            json!({"op": {"activate": {"query": "hello", "state": null}}}),
        )
        .unwrap_err();
        assert_eq!(null.to_string(), "arguments must not contain null values");

        let multiple_operations = parse_tool_call(
            "prj_test",
            MEMORY_TOOL_NAME,
            json!({"op": {"activate": {"query": "hello"}, "load": {"ids": ["mem_test"]}}}),
        )
        .unwrap_err();
        assert!(
            multiple_operations
                .to_string()
                .contains("invalid memory arguments")
        );
    }

    #[test]
    fn store_create_builds_the_existing_typed_draft_request() {
        let request = parse_tool_call(
            "prj_test",
            MEMORY_TOOL_NAME,
            json!({
                "op": {
                    "store": {
                        "resource": "memory",
                        "create": {"path": "workflow/release.md", "body": "# Release"}
                    }
                }
            }),
        )
        .unwrap();

        let AgentRuntimeRequest::Store(request) = request else {
            panic!("unexpected request variant");
        };
        assert_eq!(request.scope, DaemonDraftScope::Project);
        assert_eq!(request.resource, DaemonDraftResourceKind::Memory);
        assert_eq!(request.source, Some(DaemonDraftOperationSource::McpStore));
        assert!(request.op.create.is_some());
    }
}
