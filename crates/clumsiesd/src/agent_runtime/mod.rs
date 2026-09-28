//! Thin Agent-facing protocol adapters for the resident daemon.
//!
//! These modules deliberately contain no daemon state, database setup, model
//! loading, or background workers. They translate bounded MCP/Hook inputs into
//! the daemon's typed IPC contracts so a short-lived `clumsiesd` process can
//! remain a protocol proxy only.

pub mod mcp;
pub mod mcp_contract;

use crate::{
    AgentRuntimeIdentity, DaemonError, DaemonIpcClient, DaemonIpcRequest, DaemonIpcResponse,
};

/// Internal wire/semantic compatibility revision; bump for breaking Agent IPC changes.
pub const AGENT_RUNTIME_PROTOCOL_REVISION: u32 = 1;
/// Build provenance for diagnostics, independent of protocol compatibility.
pub const AGENT_RUNTIME_BUILD_ID: &str = env!("CLUMSIES_AGENT_RUNTIME_BUILD_ID");

/// Returns the identity compiled into this proxy or resident daemon.
pub fn current_identity() -> AgentRuntimeIdentity {
    AgentRuntimeIdentity {
        protocol_revision: AGENT_RUNTIME_PROTOCOL_REVISION,
        build_id: AGENT_RUNTIME_BUILD_ID.to_owned(),
    }
}

/// Checks compatibility for both proxy startup and resident request dispatch.
///
/// # Errors
/// Returns `agent_runtime_mismatch` when the peer uses a different protocol revision.
pub fn validate_identity(identity: &AgentRuntimeIdentity) -> Result<(), DaemonError> {
    if identity.protocol_revision == AGENT_RUNTIME_PROTOCOL_REVISION {
        return Ok(());
    }
    Err(DaemonError::State {
        code: "agent_runtime_mismatch",
        // Only the typed numeric revision is reflected; build IDs remain diagnostic metadata.
        message: format!(
            "Agent protocol revision {} is incompatible with local revision {}; update Clumsies and reconnect the Clumsies Agent integration (restart the Agent host if reconnection is unavailable). The request was not executed",
            identity.protocol_revision, AGENT_RUNTIME_PROTOCOL_REVISION
        ),
    })
}

/// Returns whether an IPC method belongs to the Agent protocol surface.
///
/// These method names are intentionally distinct from the Desktop aliases for
/// the four operations used by both clients. Requiring a compatible protocol marker
/// prevents unmarked legacy proxies and incompatible requests from reaching
/// business dispatch, including after a resident upgrade.
pub(crate) fn method_requires_identity(method: &str) -> bool {
    matches!(
        method,
        "resolve_project_binding" | "activate_memory" | "load_memory" | "store_draft_operation"
    )
}

/// Backend boundary used by the short-lived Agent protocol adapters.
///
/// Implementations receive only requests that have already been decoded into
/// an [`mcp_contract::AgentRuntimeRequest`]. This prevents Agent-supplied JSON
/// from bypassing the MCP-specific contract and becoming daemon domain input.
pub trait AgentRuntimeBackend {
    fn execute(
        &self,
        request: mcp_contract::AgentRuntimeRequest,
    ) -> Result<DaemonIpcResponse, DaemonError>;

    fn guidelines_path(&self) -> Result<Option<String>, DaemonError> {
        Ok(None)
    }

    fn active_project_id(&self) -> Result<Option<String>, DaemonError> {
        Ok(None)
    }
}

impl AgentRuntimeBackend for DaemonIpcClient {
    fn execute(
        &self,
        request: mcp_contract::AgentRuntimeRequest,
    ) -> Result<DaemonIpcResponse, DaemonError> {
        self.call(request.into_ipc_request()?)
    }

    fn guidelines_path(&self) -> Result<Option<String>, DaemonError> {
        let resp = self.project_config()?;
        Ok(resp.memory_guidelines_path)
    }

    fn active_project_id(&self) -> Result<Option<String>, DaemonError> {
        let resp = self.project_config()?;
        Ok(resp.project_id)
    }
}

impl mcp_contract::AgentRuntimeRequest {
    fn into_ipc_request(self) -> Result<DaemonIpcRequest, DaemonError> {
        let (method, payload) = match self {
            Self::Activate(request) => ("activate_memory", serde_json::to_value(request)?),
            Self::Load(request) => ("load_memory", serde_json::to_value(request)?),
            Self::Store(request) => ("store_draft_operation", serde_json::to_value(request)?),
        };
        Ok(DaemonIpcRequest::new(method, payload))
    }
}

#[cfg(test)]
mod tests {
    use crate::{ActivateMemoryRequest, AgentRuntimeIdentity};

    use super::{
        AGENT_RUNTIME_PROTOCOL_REVISION, current_identity, mcp_contract::AgentRuntimeRequest,
        validate_identity,
    };

    #[test]
    fn typed_runtime_request_maps_to_the_existing_ipc_method() {
        let request = AgentRuntimeRequest::Activate(ActivateMemoryRequest {
            project_id: "prj_test".to_owned(),
            query: "release identity".to_owned(),
            state: None,
        })
        .into_ipc_request()
        .unwrap();

        assert_eq!(request.method, "activate_memory");
        assert_eq!(request.payload["project_id"], "prj_test");
        assert_eq!(request.payload["query"], "release identity");
    }

    #[test]
    fn compatibility_depends_on_protocol_not_build() {
        assert!(validate_identity(&current_identity()).is_ok());
        assert!(
            validate_identity(&AgentRuntimeIdentity {
                protocol_revision: AGENT_RUNTIME_PROTOCOL_REVISION,
                build_id: "different-build".to_owned(),
            })
            .is_ok()
        );
        for revision in [0, AGENT_RUNTIME_PROTOCOL_REVISION + 1] {
            for build_id in [current_identity().build_id, "untrusted-build".to_owned()] {
                let error = validate_identity(&AgentRuntimeIdentity {
                    protocol_revision: revision,
                    build_id,
                })
                .unwrap_err();
                assert!(error.to_string().contains(&format!("revision {revision}")));
                assert!(error.to_string().contains("reconnect"));
                assert!(!error.to_string().contains("untrusted-build"));
            }
        }
    }

    #[test]
    fn agent_methods_require_identity_while_health_and_desktop_aliases_do_not() {
        for method in [
            "resolve_project_binding",
            "activate_memory",
            "load_memory",
            "store_draft_operation",
        ] {
            assert!(super::method_requires_identity(method), "{method}");
        }
        for method in ["health", "desktop_store_draft_operation"] {
            assert!(!super::method_requires_identity(method), "{method}");
        }
    }
}
