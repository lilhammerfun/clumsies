use crate::{
    ActivateMemoryRequest, ActivateMemoryResponse, AgentRuntimeIdentity, ClearRetrievalRunsRequest,
    ClearRetrievalRunsResponse, CreateEvaluationCaseRequest, DaemonCodexPluginRequest,
    DaemonCodexPluginStatus, DaemonDraftDetail, DaemonDraftDetailRequest, DaemonDraftListQuery,
    DaemonDraftListResponse, DaemonDraftOperationRequest, DaemonDraftOperationResponse,
    DaemonError, DaemonHealth, DaemonIpcRequest, DaemonIpcResponse, DaemonIpcService,
    DaemonLegacyAgentAdapterInspectionRequest, DaemonLegacyAgentAdapterInspectionResponse,
    DaemonProjectAgentAdapter, DaemonProjectAgentAdapterInstallRequest,
    DaemonProjectAgentAdapterListRequest, DaemonProjectAgentAdapterListResponse,
    DaemonProjectAgentAdapterRemoveRequest, DaemonProjectAgentAdapterRemoveResponse,
    DaemonProjectBinding, DaemonProjectBindingListRequest, DaemonProjectBindingListResponse,
    DaemonProjectBindingRemoveRequest, DaemonProjectBindingRemoveResponse,
    DaemonProjectBindingReplaceRequest, DaemonProjectBindingResolveRequest,
    DaemonProjectCacheClearRequest, DaemonProjectCheckout, DaemonProjectCheckoutRequest,
    DaemonProjectConfig, DaemonProjectConfigUpdateRequest, DaemonProjectSelectionRequest,
    DaemonProjectStorage, DaemonProjectStorageMove, DaemonProjectStorageMoveRequest,
    DaemonProjectStorageReplaceRequest, DaemonProjectStorageRequest,
    DaemonProjectStorageResetRequest, DaemonRetryResponse, DaemonServerRequest,
    DaemonServerResponse, DaemonSyncRetryRequest, DaemonSyncStatus, EvaluationCaseDetail,
    ExportEvaluationSetRequest, ExportEvaluationSetResponse, GetRecallFragmentRequest,
    GetRecallFragmentResponse, ListRecallsRequest, ListRecallsResponse, LoadMemoryRequest,
    LoadMemoryResponse, RecordAgentRunEventRequest, RecordAgentRunEventResponse,
    ResolveEvaluationCaseRequest, RetrievalRunDetail, RetrievalRunListRequest,
    RetrievalRunListResponse, RetrievalRunRequest, SearchIndexProjectRequest, SearchIndexStatus,
};
use std::time::Duration;

const DEFAULT_IPC_TIMEOUT: Duration = Duration::from_secs(65);

#[derive(Clone, Debug)]
pub struct DaemonIpcClient {
    service_name: String,
    agent_runtime: Option<AgentRuntimeIdentity>,
    request_timeout: Duration,
}

impl DaemonIpcClient {
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            agent_runtime: None,
            request_timeout: DEFAULT_IPC_TIMEOUT,
        }
    }

    /// Creates the IPC client used by short-lived MCP and Hook proxies.
    /// Every request sent through this client is marked with the same runtime
    /// identity so the resident daemon can validate each dispatch separately.
    pub fn for_agent_runtime(
        service_name: impl Into<String>,
        identity: AgentRuntimeIdentity,
    ) -> Self {
        Self {
            service_name: service_name.into(),
            agent_runtime: Some(identity),
            request_timeout: DEFAULT_IPC_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, request_timeout: Duration) -> Self {
        self.request_timeout = request_timeout;
        self
    }

    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    pub fn call(&self, request: DaemonIpcRequest) -> Result<DaemonIpcResponse, DaemonError> {
        platform::call(
            &self.service_name,
            self.prepare_request(request),
            self.request_timeout,
        )
    }

    fn prepare_request(&self, mut request: DaemonIpcRequest) -> DaemonIpcRequest {
        if let Some(identity) = &self.agent_runtime {
            request.agent_runtime = Some(identity.clone());
        }
        request
    }

    pub fn health(&self) -> Result<DaemonHealth, DaemonError> {
        self.call(DaemonIpcRequest::empty("health"))?.into_payload()
    }

    pub fn project_config(&self) -> Result<DaemonProjectConfig, DaemonError> {
        self.call(DaemonIpcRequest::empty("project_config"))?
            .into_payload()
    }

    pub fn replace_project_config(
        &self,
        request: DaemonProjectConfigUpdateRequest,
    ) -> Result<DaemonProjectConfig, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "replace_project_config",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn select_project(
        &self,
        request: DaemonProjectSelectionRequest,
    ) -> Result<DaemonProjectConfig, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "select_project",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn resolve_project_binding(
        &self,
        request: DaemonProjectBindingResolveRequest,
    ) -> Result<DaemonProjectBinding, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "resolve_project_binding",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn list_project_bindings(
        &self,
        request: DaemonProjectBindingListRequest,
    ) -> Result<DaemonProjectBindingListResponse, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "list_project_bindings",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn replace_project_binding(
        &self,
        request: DaemonProjectBindingReplaceRequest,
    ) -> Result<DaemonProjectBinding, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "replace_project_binding",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn remove_project_binding(
        &self,
        request: DaemonProjectBindingRemoveRequest,
    ) -> Result<DaemonProjectBindingRemoveResponse, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "remove_project_binding",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn list_project_agent_adapters(
        &self,
        request: DaemonProjectAgentAdapterListRequest,
    ) -> Result<DaemonProjectAgentAdapterListResponse, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "list_project_agent_adapters",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn list_all_project_agent_adapters(
        &self,
    ) -> Result<DaemonProjectAgentAdapterListResponse, DaemonError> {
        self.call(DaemonIpcRequest::empty("list_all_project_agent_adapters"))?
            .into_payload()
    }

    pub fn inspect_legacy_agent_adapters(
        &self,
        request: DaemonLegacyAgentAdapterInspectionRequest,
    ) -> Result<DaemonLegacyAgentAdapterInspectionResponse, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "inspect_legacy_agent_adapters",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn inspect_codex_plugin(
        &self,
        request: DaemonCodexPluginRequest,
    ) -> Result<DaemonCodexPluginStatus, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "inspect_codex_plugin",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn reconcile_codex_plugin(
        &self,
        request: DaemonCodexPluginRequest,
    ) -> Result<DaemonCodexPluginStatus, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "reconcile_codex_plugin",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn install_project_agent_adapter(
        &self,
        request: DaemonProjectAgentAdapterInstallRequest,
    ) -> Result<DaemonProjectAgentAdapter, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "install_project_agent_adapter",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn remove_project_agent_adapter(
        &self,
        request: DaemonProjectAgentAdapterRemoveRequest,
    ) -> Result<DaemonProjectAgentAdapterRemoveResponse, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "remove_project_agent_adapter",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn sync_status(&self) -> Result<DaemonSyncStatus, DaemonError> {
        self.call(DaemonIpcRequest::empty("sync_status"))?
            .into_payload()
    }

    pub fn project_storage(
        &self,
        request: DaemonProjectStorageRequest,
    ) -> Result<DaemonProjectStorage, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "project_storage",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn replace_project_storage(
        &self,
        request: DaemonProjectStorageReplaceRequest,
    ) -> Result<DaemonProjectStorageMove, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "replace_project_storage",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn project_storage_move(
        &self,
        request: DaemonProjectStorageMoveRequest,
    ) -> Result<DaemonProjectStorageMove, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "project_storage_move",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn reset_project_storage(
        &self,
        request: DaemonProjectStorageResetRequest,
    ) -> Result<DaemonProjectStorageMove, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "reset_project_storage",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn clear_project_cache(
        &self,
        request: DaemonProjectCacheClearRequest,
    ) -> Result<DaemonProjectStorage, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "clear_project_cache",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn project_checkout(
        &self,
        request: DaemonProjectCheckoutRequest,
    ) -> Result<DaemonProjectCheckout, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "project_checkout",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn activate_memory(
        &self,
        request: ActivateMemoryRequest,
    ) -> Result<ActivateMemoryResponse, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "activate_memory",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn load_memory(
        &self,
        request: LoadMemoryRequest,
    ) -> Result<LoadMemoryResponse, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "load_memory",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn record_agent_run_event(
        &self,
        request: RecordAgentRunEventRequest,
    ) -> Result<RecordAgentRunEventResponse, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "record_agent_run_event",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn search_index_status(
        &self,
        request: SearchIndexProjectRequest,
    ) -> Result<SearchIndexStatus, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "search_index_status",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn rebuild_search_index(
        &self,
        request: SearchIndexProjectRequest,
    ) -> Result<SearchIndexStatus, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "rebuild_search_index",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn list_retrieval_runs(
        &self,
        request: RetrievalRunListRequest,
    ) -> Result<RetrievalRunListResponse, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "list_retrieval_runs",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn get_retrieval_run(
        &self,
        request: RetrievalRunRequest,
    ) -> Result<RetrievalRunDetail, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "get_retrieval_run",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn list_recalls(
        &self,
        request: ListRecallsRequest,
    ) -> Result<ListRecallsResponse, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "list_recalls",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn get_recall_fragment(
        &self,
        request: GetRecallFragmentRequest,
    ) -> Result<GetRecallFragmentResponse, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "get_recall_fragment",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn create_evaluation_case(
        &self,
        request: CreateEvaluationCaseRequest,
    ) -> Result<EvaluationCaseDetail, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "create_evaluation_case",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn resolve_evaluation_case(
        &self,
        request: ResolveEvaluationCaseRequest,
    ) -> Result<EvaluationCaseDetail, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "resolve_evaluation_case",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn clear_retrieval_runs(
        &self,
        request: ClearRetrievalRunsRequest,
    ) -> Result<ClearRetrievalRunsResponse, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "clear_retrieval_runs",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn export_evaluation_set(
        &self,
        request: ExportEvaluationSetRequest,
    ) -> Result<ExportEvaluationSetResponse, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "export_evaluation_set",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn retry_sync(
        &self,
        request: DaemonSyncRetryRequest,
    ) -> Result<DaemonRetryResponse, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "retry_sync",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn list_drafts(
        &self,
        query: DaemonDraftListQuery,
    ) -> Result<DaemonDraftListResponse, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "list_drafts",
            serde_json::to_value(query)?,
        ))?
        .into_payload()
    }

    pub fn get_draft(&self, draft_id: impl Into<String>) -> Result<DaemonDraftDetail, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "get_draft",
            serde_json::to_value(DaemonDraftDetailRequest {
                draft_id: draft_id.into(),
            })?,
        ))?
        .into_payload()
    }

    pub fn store_draft_operation(
        &self,
        request: DaemonDraftOperationRequest,
    ) -> Result<DaemonDraftOperationResponse, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "store_draft_operation",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }

    pub fn server_request(
        &self,
        request: DaemonServerRequest,
    ) -> Result<DaemonServerResponse, DaemonError> {
        self.call(DaemonIpcRequest::new(
            "server_request",
            serde_json::to_value(request)?,
        ))?
        .into_payload()
    }
}

pub struct DaemonIpcServer {
    inner: platform::DaemonIpcServerInner,
}

fn validate_agent_runtime_request(request: &DaemonIpcRequest) -> Result<(), DaemonError> {
    match request.agent_runtime.as_ref() {
        Some(identity) => crate::agent_runtime::validate_identity(identity),
        None if crate::agent_runtime::method_requires_identity(&request.method) => {
            Err(DaemonError::State {
                code: "agent_runtime_mismatch",
                message:
                    "Agent proxy runtime identity is missing; restart Clumsies and the Agent host"
                        .to_owned(),
            })
        }
        None => Ok(()),
    }
}

impl DaemonIpcServer {
    pub fn start(
        service_name: impl Into<String>,
        service: DaemonIpcService,
    ) -> Result<Self, DaemonError> {
        Ok(Self {
            inner: platform::DaemonIpcServerInner::start(service_name.into(), service)?,
        })
    }

    pub fn service_name(&self) -> &str {
        self.inner.service_name()
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;

    pub fn call(
        service_name: &str,
        _request: DaemonIpcRequest,
        _timeout: Duration,
    ) -> Result<DaemonIpcResponse, DaemonError> {
        Err(DaemonError::Ipc(format!(
            "local daemon IPC is only implemented on macOS; requested XPC Mach service {service_name}"
        )))
    }

    pub struct DaemonIpcServerInner {
        service_name: String,
    }

    impl DaemonIpcServerInner {
        pub fn start(
            service_name: String,
            _service: DaemonIpcService,
        ) -> Result<Self, DaemonError> {
            Err(DaemonError::Ipc(format!(
                "local daemon IPC is only implemented on macOS; requested XPC Mach service {service_name}"
            )))
        }

        pub fn service_name(&self) -> &str {
            &self.service_name
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::{CStr, CString, c_char, c_void};
    use std::ptr;

    use block2::RcBlock;
    use tokio::runtime::Handle;

    use super::*;

    type XpcObject = *mut c_void;
    type XpcConnection = *mut c_void;
    type XpcType = *const c_void;
    type DispatchQueue = *mut c_void;

    const XPC_CONNECTION_MACH_SERVICE_LISTENER: u64 = 1;
    const REQUEST_JSON_KEY: &str = "request_json";
    const RESPONSE_JSON_KEY: &str = "response_json";

    #[link(name = "System")]
    unsafe extern "C" {
        static _xpc_type_connection: c_void;
        static _xpc_type_dictionary: c_void;
        static _xpc_type_error: c_void;

        fn xpc_connection_create_mach_service(
            name: *const c_char,
            targetq: DispatchQueue,
            flags: u64,
        ) -> XpcConnection;
        fn xpc_connection_set_event_handler(connection: XpcConnection, handler: *mut c_void);
        fn xpc_connection_activate(connection: XpcConnection);
        fn xpc_connection_cancel(connection: XpcConnection);
        fn xpc_connection_send_message(connection: XpcConnection, message: XpcObject);
        fn xpc_connection_send_message_with_reply_sync(
            connection: XpcConnection,
            message: XpcObject,
        ) -> XpcObject;

        fn xpc_dictionary_create(
            keys: *const *const c_char,
            values: *const XpcObject,
            count: usize,
        ) -> XpcObject;
        fn xpc_dictionary_create_reply(original: XpcObject) -> XpcObject;
        fn xpc_dictionary_set_string(xdict: XpcObject, key: *const c_char, value: *const c_char);
        fn xpc_dictionary_get_string(xdict: XpcObject, key: *const c_char) -> *const c_char;

        fn xpc_get_type(object: XpcObject) -> XpcType;
        fn xpc_retain(object: XpcObject) -> XpcObject;
        fn xpc_release(object: XpcObject);
    }

    pub fn call(
        service_name: &str,
        request: DaemonIpcRequest,
        timeout: Duration,
    ) -> Result<DaemonIpcResponse, DaemonError> {
        let service_name = service_name.to_owned();
        let request_json = serde_json::to_string(&request)?;
        let (connection_sender, connection_receiver) = std::sync::mpsc::sync_channel(1);
        let (result_sender, result_receiver) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let service_name = match CString::new(service_name) {
                Ok(service_name) => service_name,
                Err(error) => {
                    let error = format!("invalid XPC service name: {error}");
                    let _ = connection_sender.send(Err(error.clone()));
                    let _ = result_sender.send(Err(error));
                    return;
                }
            };
            let connection = match unsafe {
                XpcConnectionHandle::new(xpc_connection_create_mach_service(
                    service_name.as_ptr(),
                    ptr::null_mut(),
                    0,
                ))
            } {
                Ok(connection) => connection,
                Err(error) => {
                    let error = error.to_string();
                    let _ = connection_sender.send(Err(error.clone()));
                    let _ = result_sender.send(Err(error));
                    return;
                }
            };
            let handler = RcBlock::new(|_event: XpcObject| {});
            unsafe {
                xpc_connection_set_event_handler(
                    connection.as_ptr(),
                    RcBlock::as_ptr(&handler).cast(),
                );
                xpc_connection_activate(connection.as_ptr());
            }
            let parent_reference = unsafe { xpc_retain(connection.as_ptr()) } as usize;
            if connection_sender.send(Ok(parent_reference)).is_err() {
                unsafe { xpc_release(parent_reference as XpcObject) };
                return;
            }
            let result = (|| -> Result<String, String> {
                let message = xpc_dictionary_with_json(REQUEST_JSON_KEY, &request_json)
                    .map_err(|error| error.to_string())?;
                let reply = unsafe {
                    XpcOwnedObject::new(xpc_connection_send_message_with_reply_sync(
                        connection.as_ptr(),
                        message.as_ptr(),
                    ))
                    .map_err(|error| error.to_string())?
                };
                if unsafe { object_type(reply.as_ptr()) } == unsafe { xpc_error_type() } {
                    return Err("XPC returned an error object".to_owned());
                }
                xpc_dictionary_string(reply.as_ptr(), RESPONSE_JSON_KEY)
                    .map_err(|error| error.to_string())
            })();
            let _ = result_sender.send(result);
        });
        let started = std::time::Instant::now();
        let connection_address = match connection_receiver.recv_timeout(timeout) {
            Ok(Ok(connection_address)) => connection_address,
            Ok(Err(error)) => return Err(DaemonError::Ipc(error)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                return Err(DaemonError::Ipc(format!(
                    "XPC connection setup timed out after {} ms",
                    timeout.as_millis()
                )));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(DaemonError::Ipc(
                    "XPC connection setup channel disconnected".to_owned(),
                ));
            }
        };
        let remaining = timeout.saturating_sub(started.elapsed());
        let response = result_receiver.recv_timeout(remaining);
        if matches!(response, Err(std::sync::mpsc::RecvTimeoutError::Timeout)) {
            unsafe { xpc_connection_cancel(connection_address as XpcConnection) };
        }
        unsafe { xpc_release(connection_address as XpcObject) };
        let response_json = match response {
            Ok(Ok(response_json)) => response_json,
            Ok(Err(error)) => return Err(DaemonError::Ipc(error)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Cancelling a connection wakes an in-flight synchronous XPC
                // reply wait with an error object. The helper thread keeps the
                // blocking C call off the Hook/MCP protocol thread.
                return Err(DaemonError::Ipc(format!(
                    "XPC request timed out after {} ms",
                    timeout.as_millis()
                )));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(DaemonError::Ipc(
                    "XPC reply channel disconnected".to_owned(),
                ));
            }
        };
        serde_json::from_str(&response_json).map_err(DaemonError::from)
    }

    pub struct DaemonIpcServerInner {
        service_name: String,
        _listener: XpcConnectionHandle,
        _handler: RcBlock<dyn Fn(XpcObject) + 'static>,
    }

    impl DaemonIpcServerInner {
        pub fn start(service_name: String, service: DaemonIpcService) -> Result<Self, DaemonError> {
            let runtime = Handle::try_current().map_err(|error| {
                DaemonError::Ipc(format!("tokio runtime is required for XPC server: {error}"))
            })?;
            let service_name_c = CString::new(service_name.as_str())
                .map_err(|error| DaemonError::Ipc(format!("invalid XPC service name: {error}")))?;
            let listener = unsafe {
                XpcConnectionHandle::new(xpc_connection_create_mach_service(
                    service_name_c.as_ptr(),
                    ptr::null_mut(),
                    XPC_CONNECTION_MACH_SERVICE_LISTENER,
                ))?
            };
            let handler = RcBlock::new(move |peer: XpcObject| {
                if unsafe { object_type(peer) } != unsafe { xpc_connection_type() } {
                    return;
                }
                accept_peer(peer.cast(), service.clone(), runtime.clone());
            });
            unsafe {
                xpc_connection_set_event_handler(
                    listener.as_ptr(),
                    RcBlock::as_ptr(&handler).cast(),
                );
                xpc_connection_activate(listener.as_ptr());
            }
            Ok(Self {
                service_name,
                _listener: listener,
                _handler: handler,
            })
        }

        pub fn service_name(&self) -> &str {
            &self.service_name
        }
    }

    fn accept_peer(peer: XpcConnection, service: DaemonIpcService, runtime: Handle) {
        let peer_handler = RcBlock::new(move |message: XpcObject| {
            if unsafe { object_type(message) } != unsafe { xpc_dictionary_type() } {
                return;
            }
            // Never run request handling on this callback thread: libxpc
            // dispatches connection event handlers onto a GCD global queue
            // whose threads have only a 512 KiB stack, while request handling
            // (e.g. retry_sync -> commit_sync -> HTTPS) unfolds a deeply
            // nested future chain that overflows it with a silent SIGILL.
            // Retain the message, hand the work to a tokio worker, and let the
            // worker build and send the reply.
            // Raw pointers are moved across threads as usize (same pattern as
            // the connection retention below), then cast back inside the task.
            let message = unsafe { xpc_retain(message) } as usize;
            let service = service.clone();
            let peer = unsafe { xpc_retain(peer) } as usize;
            runtime.spawn(async move {
                let message = SendXpc(message as *mut c_void);
                let peer = SendXpc(peer as *mut c_void);
                let response_json = dispatch_message(&service, message).await;
                if let (Ok(response_json), Ok(reply)) = (response_json, unsafe {
                    XpcOwnedObject::new(xpc_dictionary_create_reply(message.0))
                }) && set_xpc_string(reply.as_ptr(), RESPONSE_JSON_KEY, &response_json).is_ok()
                {
                    unsafe {
                        xpc_connection_send_message(peer.0, reply.as_ptr());
                    }
                }
                unsafe {
                    xpc_release(peer.0);
                    xpc_release(message.0);
                }
            });
        });
        unsafe {
            xpc_connection_set_event_handler(peer, RcBlock::as_ptr(&peer_handler).cast());
            xpc_connection_activate(peer);
        }
    }

    /// Handles one XPC request on a tokio worker thread. The GCD callback
    /// thread only retains the message and spawns this task, so request
    /// handling never runs on the 512 KiB dispatch stack.
    async fn dispatch_message(
        service: &DaemonIpcService,
        message: SendXpc,
    ) -> Result<String, DaemonError> {
        let request_json = xpc_dictionary_string(message.0, REQUEST_JSON_KEY)?;
        let request: DaemonIpcRequest = serde_json::from_str(&request_json)?;
        let response = match validate_agent_runtime_request(&request) {
            Ok(()) => service.dispatch(request).await,
            Err(error) => DaemonIpcResponse::from_result(Err(error)),
        };
        serde_json::to_string(&response).map_err(DaemonError::from)
    }

    /// XPC objects are reference-counted and documented as safe to use from
    /// any thread; the raw pointer only needs an explicit Send marker to move
    /// into the tokio task that sends the reply.
    #[derive(Clone, Copy)]
    struct SendXpc(XpcObject);

    // Safety: libxpc objects may be retained/released/sent from any thread.
    unsafe impl Send for SendXpc {}

    fn xpc_dictionary_with_json(key: &str, value: &str) -> Result<XpcOwnedObject, DaemonError> {
        let object =
            unsafe { XpcOwnedObject::new(xpc_dictionary_create(ptr::null(), ptr::null(), 0))? };
        set_xpc_string(object.as_ptr(), key, value)?;
        Ok(object)
    }

    fn set_xpc_string(object: XpcObject, key: &str, value: &str) -> Result<(), DaemonError> {
        let key = CString::new(key)
            .map_err(|error| DaemonError::Ipc(format!("invalid XPC key: {error}")))?;
        let value = CString::new(value)
            .map_err(|error| DaemonError::Ipc(format!("invalid XPC string value: {error}")))?;
        unsafe {
            xpc_dictionary_set_string(object, key.as_ptr(), value.as_ptr());
        }
        Ok(())
    }

    fn xpc_dictionary_string(object: XpcObject, key: &str) -> Result<String, DaemonError> {
        if unsafe { object_type(object) } != unsafe { xpc_dictionary_type() } {
            return Err(DaemonError::Ipc(
                "expected XPC dictionary response".to_owned(),
            ));
        }
        let key = CString::new(key)
            .map_err(|error| DaemonError::Ipc(format!("invalid XPC key: {error}")))?;
        let value = unsafe { xpc_dictionary_get_string(object, key.as_ptr()) };
        if value.is_null() {
            return Err(DaemonError::Ipc(format!(
                "missing XPC string field: {}",
                key.to_string_lossy()
            )));
        }
        Ok(unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned())
    }

    unsafe fn object_type(object: XpcObject) -> XpcType {
        unsafe { xpc_get_type(object) }
    }

    unsafe fn xpc_connection_type() -> XpcType {
        &raw const _xpc_type_connection as XpcType
    }

    unsafe fn xpc_dictionary_type() -> XpcType {
        &raw const _xpc_type_dictionary as XpcType
    }

    unsafe fn xpc_error_type() -> XpcType {
        &raw const _xpc_type_error as XpcType
    }

    struct XpcOwnedObject {
        object: XpcObject,
    }

    impl XpcOwnedObject {
        unsafe fn new(object: XpcObject) -> Result<Self, DaemonError> {
            if object.is_null() {
                Err(DaemonError::Ipc("XPC returned a null object".to_owned()))
            } else {
                Ok(Self { object })
            }
        }

        fn as_ptr(&self) -> XpcObject {
            self.object
        }
    }

    impl Drop for XpcOwnedObject {
        fn drop(&mut self) {
            unsafe {
                xpc_release(self.object);
            }
        }
    }

    struct XpcConnectionHandle {
        connection: XpcConnection,
    }

    impl XpcConnectionHandle {
        unsafe fn new(connection: XpcConnection) -> Result<Self, DaemonError> {
            if connection.is_null() {
                Err(DaemonError::Ipc(
                    "XPC returned a null connection".to_owned(),
                ))
            } else {
                Ok(Self { connection })
            }
        }

        fn as_ptr(&self) -> XpcConnection {
            self.connection
        }
    }

    impl Drop for XpcConnectionHandle {
        fn drop(&mut self) {
            unsafe {
                xpc_connection_cancel(self.connection);
                xpc_release(self.connection.cast());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_requests_keep_the_legacy_envelope_shape() {
        let request: DaemonIpcRequest =
            serde_json::from_str(r#"{"method":"health","payload":{}}"#).unwrap();
        assert_eq!(request.agent_runtime, None);
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({"method": "health", "payload": {}})
        );
    }

    #[test]
    fn agent_client_marks_every_prepared_request() {
        let identity = crate::agent_runtime::current_identity();
        let client = DaemonIpcClient::for_agent_runtime("ai.clumsies.test", identity.clone());

        for request in [
            DaemonIpcRequest::empty("health"),
            DaemonIpcRequest::new("resolve_project_binding", serde_json::json!({})),
            DaemonIpcRequest::new("activate_memory", serde_json::json!({})),
        ] {
            assert_eq!(
                client.prepare_request(request).agent_runtime,
                Some(identity.clone())
            );
        }
    }

    #[test]
    fn every_agent_method_rejects_a_missing_or_stale_runtime_marker() {
        let stale = AgentRuntimeIdentity {
            protocol_revision: crate::agent_runtime::AGENT_RUNTIME_PROTOCOL_REVISION,
            build_id: "stale-test-build".to_owned(),
        };
        for method in [
            "resolve_project_binding",
            "activate_memory",
            "load_memory",
            "store_draft_operation",
            "record_agent_run_event",
        ] {
            let missing = DaemonIpcRequest::empty(method);
            assert!(matches!(
                validate_agent_runtime_request(&missing),
                Err(DaemonError::State {
                    code: "agent_runtime_mismatch",
                    ..
                })
            ));
            let mut stale_request = DaemonIpcRequest::empty(method);
            stale_request.agent_runtime = Some(stale.clone());
            assert!(matches!(
                validate_agent_runtime_request(&stale_request),
                Err(DaemonError::State {
                    code: "agent_runtime_mismatch",
                    ..
                })
            ));
        }
    }

    #[test]
    fn unmarked_health_and_desktop_aliases_remain_available() {
        for method in ["health", "desktop_store_draft_operation"] {
            assert!(validate_agent_runtime_request(&DaemonIpcRequest::empty(method)).is_ok());
        }
    }

    #[test]
    fn missing_mach_service_returns_an_ipc_error() {
        let client = DaemonIpcClient::new("ai.clumsies.daemon.missing-test-service");

        let error = client.health().unwrap_err();

        assert!(matches!(error, DaemonError::Ipc(_)));
    }
}
