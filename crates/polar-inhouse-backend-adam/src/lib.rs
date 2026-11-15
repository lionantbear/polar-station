//! Adam backend driver primitives for Polar inhouse agents.
//!
//! This crate owns Adam-specific backend configuration, projection metadata,
//! llama transports, OpenAI OAuth request/event parsing, and the
//! connector-driven runner. Upstream runtime crates are expected to wire these
//! primitives into the shared inhouse scheduler without moving Adam semantics
//! into `polar-serve` or `polar-service-kernel`.

pub mod backend_runtime;
pub mod cache_scheduler;
pub mod causal_identity;
mod compaction_physical_port;
pub mod config;
pub mod engine_manager;
pub mod engine_store;
pub mod llama;
pub mod llama_engine_operations;
pub mod openai;
pub mod projection_store;
pub mod runner;
pub mod status;
pub mod tool_bridge;

pub use backend_runtime::{
    parse_adam_managed_endpoint_declarations, AdamBackendRuntime, AdamBackendRuntimeConfig,
    AdamManagedEndpointDeclaration, AdamManagedEndpointDeclarations,
    ADAM_MANAGED_ENDPOINT_DECLARATIONS_FILE, ADAM_MANAGED_ENDPOINT_DECLARATIONS_SCHEMA_VERSION,
};

pub use cache_scheduler::{
    AdamCacheAllocationReason, AdamCacheResident, AdamCacheScheduleDecision,
    AdamCacheScheduleRequest, AdamCacheScheduler, AdamCacheSchedulerConfig, AdamWaitQueueStatus,
    CacheAffinityKey,
};
pub use causal_identity::{
    adam_tool_arguments_hash, adam_tool_invocation_id, grade_adam_causal_evidence,
    AdamCausalGradeFailure, AdamCausalIdentityMode, AdamToolInvocationEvidence,
    AdamToolInvocationStatus,
};
pub use config::{
    parse_adam_agent_config, AdamAgentConfig, AdamBackendConfig, AdamConfigError,
    AdamConnectionConfig, AdamLlmConnectionMode, LlamaConfig, LlamaTransport, OpenAiConfig,
    SlotStrategy,
};
pub use engine_manager::{
    normalize_connector_url, AdamCachePlan, AdamEngineAllocation, AdamEngineAllocationWaitConfig,
    AdamEngineControlEvidence, AdamEngineControlIntent, AdamEngineOperations,
    AdamEngineTurnPriority, AdamInferenceEngineManager, AdamLlamaEngineProbe,
    AdamPreparedCompactIntent, AdamPreparedCompactPromotion, AdamPreparedCompactResult,
    AdamSlotLease, AdamSnapshotRestoreResult, AdamSnapshotSaveResult,
};
pub use engine_store::{
    AdamEngineCompletionCommit, AdamEngineCompletionEvidence, AdamEngineStore,
    AdamKvSnapshotRecord, AdamLiveSlotResidencyRecord, AdamManagedLlamaEndpoint,
    AdamPersistentSlotLease, AdamPreparedCompactSlot, AdamPromotedPreparedLookup,
    AdamSlotLeaseCasRequest, AdamSnapshotCompatibility, AdamTurnSlotReservation,
    AdamTurnSlotReservationRequest, InMemoryAdamEngineStore,
};
pub use llama::{
    build_chat_completions_request, build_llamacpp_completion_request, build_responses_request,
    classify_llamacpp_busy, llamacpp_wire_prompt_hash, parse_chat_completions_response,
    parse_llamacpp_completion_response, parse_llamacpp_props_response,
    parse_llamacpp_slots_response, parse_responses_response, parse_strict_tool_envelope,
    AdamLocalToolEnvelopeAction, ChatCompletionOutput, LlamaCompletionOutput,
    LlamaCompletionRequest, LlamaCppSlotsClient, LlamaSlotProbe, LlamaThinkTagParser,
    LlamaWireProjection, LlamaWireProjectionRequest, OpenAiResponseOutput,
};
pub use llama_engine_operations::LlamaAdamEngineOperations;
pub use openai::{
    build_openai_headers, build_openai_request, build_openai_response_create_message,
    openai_responses_websocket_url, parse_oauth_token_file, parse_openai_event,
    AdamOAuthCredential, OpenAiRequest, OpenAiResponsesEvent, SecretString,
};
pub use projection_store::{
    AdamCacheEpoch, AdamCacheEpochLookup, AdamCacheEpochState, AdamCapabilityProfileSnapshot,
    AdamCompactionAdmissionDecision, AdamLlamaCacheEvidence, AdamPreparedCompactSlotRecord,
    AdamProjectionLookup, AdamProviderUsageCounters, AdamProviderUsageTelemetry,
    AdamRenderProjection, AdamRuntimeProjectionStore, AdamSessionBinding, AdamSessionBindingKey,
    AdamToolCallCorrelation, AdamTurnAttempt, AdamTurnAttemptStatus, AdamUsageAccountingSource,
    InMemoryAdamRuntimeProjectionStore,
};
pub use runner::{
    invalidate_adam_context_compaction_epoch, project_adam_prepared_compact_request,
    AdamAccumulatedTurn, AdamAgentRunner, AdamBackendError, AdamBusyKind,
    AdamCompactionEpochInvalidation, AdamOAuthCredentialResolver, AdamPreparedCompactProjection,
    AdamProviderConnector, AdamProviderEvent, AdamProviderEventSink, AdamProviderRequest,
    AdamProviderTurnEnd, AdamRunnerProjectionHashes, AdamThinkingBlock, AdamThinkingSource,
    AdamToolCall, AdamToolResult, AdamTurnAccumulator, AdamTurnContext, AdamTurnEventSink,
};
pub use status::{
    build_adam_engine_status, AdamEffectiveEngineCapabilities, AdamEndpointStatus,
    AdamEngineStatusReport, AdamResidencyStatus, AdamRunCompletionFacts, AdamRunReport,
};

pub fn now_unix_ms() -> i64 {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

pub(crate) fn append_path(base_url: &str, path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{base}/{path}")
}
