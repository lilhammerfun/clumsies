use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;

mod agent_adapter;
pub mod agent_runtime;
mod commit_sync;
pub mod config;
mod credentials;
mod dashboard;
pub mod diagnostics;
mod draft;
pub use dashboard::{DashboardRetrievalRequest, DashboardRetrievalStatistics};
mod ipc;
mod migration;
mod project_storage;
mod recall;
mod retrieval_history;
mod search;
mod server_client;
mod state;
mod types;
mod util;

pub use agent_adapter::{
    DaemonAgentAdapterSetting, DaemonAgentAdapterSettings, DaemonCodexPluginRequest,
    DaemonCodexPluginStatus, DaemonLegacyAgentAdapterConflict,
    DaemonLegacyAgentAdapterInspectionRequest, DaemonLegacyAgentAdapterInspectionResponse,
    DaemonProjectAgentAdapter, DaemonProjectAgentAdapterInstallRequest,
    DaemonProjectAgentAdapterListRequest, DaemonProjectAgentAdapterListResponse,
    DaemonProjectAgentAdapterRemoveRequest, DaemonProjectAgentAdapterRemoveResponse,
    DaemonSetAgentAdapterRequest, ProjectAgentAdapterDelivery, ProjectAgentAdapterKind,
};
pub use commit_sync::{
    DaemonMemoryCacheRequest, DaemonMemoryCacheState, DaemonMemoryCacheStatus,
    DaemonProjectCheckout, DaemonProjectCheckoutRequest, DaemonProjectCheckoutResource,
};
pub use config::{
    CURRENT_LOCAL_SCHEMA_VERSION, DAEMON_AGENT_LABEL, DAEMON_MACH_SERVICE_NAME,
    DEV_INSTANCE_ID_ENV, DaemonConfig, IDENTIFIER_NAMESPACE, LaunchAgentConfig,
    LaunchAgentController, ProjectConfig, SyncConfig,
};
pub(crate) use config::{
    META_DRAFT_SYNC_LAST_ATTEMPT_AT, META_DRAFT_SYNC_LAST_SUCCESS_AT, RuntimeProjectConfig,
};
pub use credentials::{
    CredentialStore, CredentialStoreError, KEYCHAIN_ACCOUNT, ServerCredentials,
    SystemCredentialStore,
};
pub(crate) use draft::{
    LocalDraftResolutionInput, drain_draft_queue, list_local_drafts, load_local_draft_detail,
    load_project_sync_status, load_sync_status, pull_draft_events, queue_retrying_operations,
    recover_interrupted_operations, resolve_local_draft,
};
pub use ipc::{DaemonIpcClient, DaemonIpcServer};
pub(crate) use migration::{
    connect_local_db, current_schema_version, load_meta_value, load_or_create_installation_id,
    load_project_config, load_server_credentials, migrate_local_db, prepare_directories,
    replace_server_credentials, reset_memory_cache_if_required, save_project_metadata,
    upsert_meta_timestamp, upsert_meta_value,
};
pub use project_storage::{
    DaemonProjectCacheClearRequest, DaemonProjectStorage, DaemonProjectStorageAvailability,
    DaemonProjectStorageMode, DaemonProjectStorageMove, DaemonProjectStorageMoveRequest,
    DaemonProjectStorageMoveState, DaemonProjectStorageReplaceRequest, DaemonProjectStorageRequest,
    DaemonProjectStorageResetRequest,
};
pub use recall::{
    AgentHost, GetRecallFragmentRequest, GetRecallFragmentResponse, GetRecallSessionRequest,
    GetRecallSessionResponse, ListRecallsRequest, ListRecallsResponse, RecallActivation,
    RecallFragment, RecallSession, RecallSessionSummary, RecallTask,
};
pub use retrieval_history::{
    ClearRetrievalRunsRequest, ClearRetrievalRunsResponse, CreateEvaluationCaseRequest,
    EvaluationCase, EvaluationCaseDetail, EvaluationCaseStatus, EvaluationEvidence,
    EvaluationEvidenceInput, EvaluationEvidenceSuggestion, ExportEvaluationSetRequest,
    ExportEvaluationSetResponse, ResolveEvaluationCaseRequest, RetrievalBenchmarkMetrics,
    RetrievalBenchmarkReport, RetrievalBenchmarkVariant, RetrievalCandidate, RetrievalDeltaAction,
    RetrievalExclusionReason, RetrievalFailureStage, RetrievalRun, RetrievalRunDetail,
    RetrievalRunListRequest, RetrievalRunListResponse, RetrievalRunRequest, RetrievalRunStatus,
    RetrievalStageLatencies,
};
pub use search::{
    ActivateMemoryRequest, ActivateMemoryResponse, ActivationAction, ActivationFragment,
    ActivationRemoval, LoadMemoryRequest, LoadMemoryResponse, LoadedMemoryResource, MemoryKind,
    SearchIndexProjectRequest, SearchIndexStatus, SearchModelStatus, SourceLocator, SourceScope,
};
pub(crate) use server_client::{
    clear_server_response_cache, decode_server_json, delete_server_json, ensure_server_success,
    execute_authenticated_server_request, filter_proxy_request_headers,
    filter_proxy_response_headers, get_server_json, is_retryable_http_status,
    load_cached_server_response, post_server_json, save_cached_server_response,
    validate_server_proxy_path,
};
pub use state::DaemonIpcService;
pub use state::DaemonState;
pub(crate) use types::ProjectConfigReadiness;
pub(crate) use types::ServerTokenRefreshResponse;
pub(crate) use types::project_binding_from_row;
pub use types::{
    AgentRuntimeIdentity, ApiError, DaemonBootstrapStatus, DaemonDraftContent, DaemonDraftDetail,
    DaemonDraftDetailRequest, DaemonDraftFreshness, DaemonDraftListQuery, DaemonDraftListResponse,
    DaemonDraftOperation, DaemonDraftOperationRecordSource, DaemonDraftOperationRequest,
    DaemonDraftOperationResponse, DaemonDraftOperationSource, DaemonDraftReconciliationStatus,
    DaemonDraftResourceKind, DaemonDraftScope, DaemonDraftSummary, DaemonError, DaemonHealth,
    DaemonIpcEndpoint, DaemonIpcRequest, DaemonIpcResponse, DaemonIpcTransport,
    DaemonLocalDraftStatus, DaemonProjectBinding, DaemonProjectBindingListRequest,
    DaemonProjectBindingListResponse, DaemonProjectBindingRemoveRequest,
    DaemonProjectBindingRemoveResponse, DaemonProjectBindingReplaceRequest,
    DaemonProjectBindingResolveRequest, DaemonProjectConfig, DaemonProjectConfigUpdateRequest,
    DaemonProjectSelectionRequest, DaemonProjectSyncRetryRequest, DaemonProjectSyncStatusRequest,
    DaemonRetryResponse, DaemonServerRequest, DaemonServerResponse, DaemonSyncRetryRequest,
    DaemonSyncStatus, DraftOperationSyncStatus, ErrorEnvelope, LaunchAgentRuntimeStatus,
    LocalDbStatus, ProjectAgentAdapterRuntimeRequirement, SyncChannelStatus, SyncRetryChannel,
    SyncState,
};
pub use types::{
    DaemonContentDraftUpdate, DaemonCreateDraftOperation, DaemonDeleteDraftOperation,
    DaemonDiscardDraftOperation, DaemonLocalDraftOperation, DaemonRenameDraftOperation,
    DaemonTextDraftUpdate, DaemonTextReplacement, DaemonUpdateDraftOperation,
};
pub(crate) use util::is_normalized_relative_path;
use util::{
    apply_exact_text_replacements, canonical_binding_root, canonical_server_url,
    canonical_workspace_directory, git_worktree_main_root, memory_kind_matches_resource,
    non_empty_string,
};
