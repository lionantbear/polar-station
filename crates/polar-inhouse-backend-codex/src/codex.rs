use crate::client::AppServerEvent;
use crate::codex_app_server_bootstrap as bootstrap_adapter;
use crate::codex_app_server_compat as upstream_compat;
use crate::thread_lifecycle::{prepare_existing, ThreadLifecycleIo, ThreadSetup};
use crate::{backend_request_debug_enabled, safe_debug_text};
use anyhow::Context;
use base64::Engine;
pub use polar_inhouse_contract::CodexRuntimeBackend;
use polar_inhouse_contract::{
    prepend_turn_prompt_to_input, InhouseAgentRunner, InhouseApprovalDecision,
    InhouseApprovalHandler, InhouseApprovalKind, InhouseApprovalRequest, InhouseBackendModel,
    InhouseBackendModelCatalog, InhouseConnectorDenyPolicy, InhouseDynamicToolHandler,
    InhouseMessageProjection, InhouseModelInputError, InhousePromptSegment, InhouseTurnControl,
    InhouseTurnEventHandler, InhouseTurnOptions, PolarPromptProjection,
    InhouseToolCallEnvelope, InhouseToolResultEnvelope,
};
use polar_inhouse_prompt::{render_codex_prompt_envelope, render_codex_turn_prompt_envelope};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, info, warn};
use upstream_compat::{
    command_execution_cwd, thread_resume_params, thread_start_params, turn_start_params,
    AskForApproval, ClientRequest, CommandExecutionApprovalDecision,
    CommandExecutionRequestApprovalParams, CommandExecutionRequestApprovalResponse, ContentItem,
    DynamicToolCallOutputContentItem, DynamicToolCallResponse,
    ErrorNotification, FileChangeApprovalDecision, FileChangeRequestApprovalParams,
    FileChangeRequestApprovalResponse, ItemCompletedNotification,
    ItemStartedNotification, JSONRPCErrorError, ModelListParams, ModelListResponse,
    ReasoningEffort, RequestId, ResponseItem, SandboxMode, SandboxPolicy, ServerNotification,
    ServerRequest, ThreadItem, ThreadItemPayload, ThreadReadParams, ThreadReadResponse, ThreadResumeInput,
    ThreadStartInput, ThreadStartResponse, ToolRequestUserInputResponse, Turn, TurnError,
    TurnInterruptParams, TurnInterruptResponse, TurnPlanStep, TurnPlanStepStatus, TurnStartInput,
    TurnStartResponse, TurnStatus, TypedRequestError, UserInput,
};

const TURN_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const IN_PROCESS_PLAN_UPDATE_TOTAL_LIMIT: usize = 8;
const IN_PROCESS_REPEATED_PLAN_SIGNATURE_LIMIT: usize = 2;
const IN_PROCESS_REPEATED_TOOL_SIGNATURE_LIMIT: usize = 2;
const IN_PROCESS_REPEATED_BUILTIN_FUNCTION_SIGNATURE_LIMIT: usize = 2;
const INLINE_IMAGE_MAX_BYTES: usize = 8 * 1024 * 1024;
const INLINE_IMAGE_MIN_DIMENSION: u32 = 8;
const INLINE_IMAGE_MAX_DIMENSION: u32 = 4096;
const INLINE_IMAGE_MAX_PIXELS: u64 = 16_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexMode {
    Agentic,
    Conversation,
}

impl CodexMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agentic => "agentic",
            Self::Conversation => "conversation",
        }
    }

    pub fn is_conversation(self) -> bool {
        self == Self::Conversation
    }
}

impl std::str::FromStr for CodexMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "agentic" => Ok(Self::Agentic),
            "conversation" => Ok(Self::Conversation),
            other => {
                anyhow::bail!("unsupported codex_mode '{other}'; expected agentic or conversation")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexPromptProjection {
    prompt_override_enabled: bool,
    base_instructions: Option<String>,
    developer_instructions: Option<String>,
    user_instructions: Option<String>,
    turn_input_prefix: Option<String>,
    message_projection: Option<InhouseMessageProjection>,
}

#[derive(Clone, Copy)]
struct CodexTurnPromptContext<'a> {
    turn_options: &'a InhouseTurnOptions,
    include_canonical_history: bool,
}

async fn prepare_codex_provider_attempt(
    turn_options: &InhouseTurnOptions,
    backend_operation: &str,
    provider_request: &Value,
) -> anyhow::Result<()> {
    turn_options
        .provider_attempt_gate
        .prepare("codex", backend_operation, provider_request)
        .await
        .context("persist Codex provider attempt before app-server contact")?;
    Ok(())
}

/// Lowers the shared provider-neutral prompt request for every Codex app-server
/// transport. Legacy fields are adapted above the backend runner boundary.
fn resolve_codex_prompt_projection(turn_options: &InhouseTurnOptions) -> CodexPromptProjection {
    let request = turn_options
        .backend_turn_request
        .as_ref()
        .expect("runner validates the shared backend request before prompt rendering");
    let thread = render_codex_prompt_envelope(&request.prompt_envelope);
    let turn = render_codex_turn_prompt_envelope(&request.prompt_envelope);
    CodexPromptProjection {
        prompt_override_enabled: thread.prompt_override_enabled,
        base_instructions: thread.base_instructions,
        developer_instructions: thread.developer_instructions,
        user_instructions: thread.user_instructions,
        turn_input_prefix: turn.input_prefix,
        message_projection: Some(request.message_projection.clone()),
    }
}

fn ensure_shared_backend_turn_request(turn_options: &InhouseTurnOptions) -> anyhow::Result<()> {
    if turn_options.backend_turn_request.is_none() {
        anyhow::bail!(
            "Codex backend requires an assembled InhouseBackendTurnRequest; legacy options must be adapted above the runner boundary"
        );
    }
    Ok(())
}

fn model_catalog_from_app_server_value(value: Value) -> anyhow::Result<InhouseBackendModelCatalog> {
    let entries = value
        .get("data")
        .and_then(Value::as_array)
        .context("Codex app-server model/list response has no data array")?;
    let mut seen = BTreeSet::new();
    let mut models = Vec::new();
    for entry in entries {
        let id = entry
            .get("model")
            .or_else(|| entry.get("id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .context("Codex app-server model/list entry has no model id")?;
        if !seen.insert(id.to_string()) {
            continue;
        }
        let thinking_levels = entry
            .get("supportedReasoningEfforts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|effort| {
                effort
                    .get("reasoningEffort")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string)
            })
            .collect();
        let default_thinking_level = entry
            .get("defaultReasoningEffort")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        models.push(InhouseBackendModel {
            id: id.to_string(),
            is_default: entry
                .get("isDefault")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            thinking_levels,
            default_thinking_level,
        });
    }
    anyhow::ensure!(
        !models.is_empty(),
        "Codex app-server model/list returned no selectable models"
    );
    Ok(InhouseBackendModelCatalog {
        models,
        provider: None,
        source: "codex_app_server".to_string(),
        endpoint: Some("model/list".to_string()),
    })
}

/// Codex app-server persists its own thread history. A fresh app-server thread
/// receives prior canonical LGP segments, while a resumed thread receives no
/// replayed segments. In both cases the current-turn pointer is retained and
/// the actual current payload remains the ordinary app-server turn input.
fn codex_turn_prompt_prefix(
    prompt: &CodexPromptProjection,
    include_canonical_history: bool,
) -> Option<String> {
    let canonical_projection = prompt.message_projection.as_ref().and_then(|projection| {
        render_codex_message_projection(projection, include_canonical_history)
    });
    let combined = [
        prompt.turn_input_prefix.as_deref(),
        canonical_projection.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n");
    (!combined.is_empty()).then_some(combined)
}

fn render_codex_message_projection(
    projection: &InhouseMessageProjection,
    include_canonical_history: bool,
) -> Option<String> {
    let segments = if include_canonical_history {
        prior_message_segments(projection)
    } else {
        Vec::new()
    };
    if segments.is_empty() && projection.current_turn_message_id.is_none() {
        return None;
    }
    let rendered = PolarPromptProjection::from_segments(
        "",
        segments,
        projection.current_turn_message_id.as_deref(),
    )
    .full_prompt;
    let rendered = rendered.trim();
    (!rendered.is_empty()).then(|| format!("Canonical LGP message projection:\n{rendered}"))
}

fn prior_message_segments(projection: &InhouseMessageProjection) -> Vec<InhousePromptSegment> {
    projection
        .segments
        .iter()
        .filter(|segment| {
            projection
                .current_turn_message_id
                .as_deref()
                .is_none_or(|current| !prompt_segment_matches_message_id(segment, current))
        })
        .cloned()
        .collect()
}

fn prompt_segment_matches_message_id(segment: &InhousePromptSegment, message_id: &str) -> bool {
    [
        segment.segment_id.strip_prefix("lgp-message:"),
        segment.source_ref.strip_prefix("lgp:message:"),
    ]
    .into_iter()
    .flatten()
    .filter_map(|value| value.rsplit_once('@').map(|(id, _)| id))
    .any(|id| id == message_id)
}

#[derive(Clone)]
pub struct CodexAgentConfig {
    pub inhouse_polar_id: String,
    pub codex_mode: CodexMode,
    pub backend: CodexRuntimeBackend,
    pub socket_path: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub model: Option<String>,
    /// Optional native ModelInfo object for the fixed conversation model.
    pub conversation_model_metadata: Option<Value>,
    pub effort: Option<String>,
    pub approval_policy: Option<String>,
    pub sandbox_policy: Option<Value>,
    /// Optional Codex config profile name carried by Polar runtime config.
    pub config_profile: Option<String>,
    pub auth_home: Option<PathBuf>,
    pub profile: Option<crate::CodexProfilePaths>,
    /// Optional explicit Codex model provider id carried by Polar runtime config.
    pub model_provider: Option<String>,
    /// Optional explicit provider base URL for OpenAI-compatible Codex providers.
    pub provider_base_url: Option<String>,
    /// OSS mode: which local provider to use ("lmstudio", etc.) or None for cloud models.
    pub oss_mode: Option<String>,
    /// LM Studio host URL for OSS mode.
    pub lmstudio_host: Option<String>,
    /// When true, enable upstream Codex memories feature for this inhouse runtime.
    pub memory_tool_enabled: bool,
    /// Optional override for Codex memories.generate_memories.
    pub generate_memories: Option<bool>,
    /// Optional override for Codex memories.use_memories.
    pub use_memories: Option<bool>,
    /// Optional override for Codex memories.min_rollout_idle_hours.
    pub min_rollout_idle_hours: Option<i64>,
    /// Optional override for Codex memories.extract_model.
    pub memory_extract_model: Option<String>,
    /// Optional override for Codex memories.consolidation_model.
    pub memory_consolidation_model: Option<String>,
    /// Host-reserved provider connectors that Codex must never resolve to.
    /// This is provider-neutral and deliberately contains no Adam engine data.
    pub connector_deny_policy: InhouseConnectorDenyPolicy,
    pub approval_handler: Option<Arc<dyn InhouseApprovalHandler>>,
}

impl CodexAgentConfig {
    pub fn managed(
        codex_mode: CodexMode,
        connector_deny_policy: InhouseConnectorDenyPolicy,
    ) -> Self {
        Self {
            inhouse_polar_id: String::new(),
            codex_mode,
            backend: CodexRuntimeBackend::InProcess,
            socket_path: None,
            cwd: None,
            model: None,
            conversation_model_metadata: None,
            effort: None,
            approval_policy: None,
            sandbox_policy: None,
            config_profile: None,
            auth_home: None,
            profile: None,
            model_provider: None,
            provider_base_url: None,
            oss_mode: None,
            lmstudio_host: None,
            memory_tool_enabled: false,
            generate_memories: None,
            use_memories: None,
            min_rollout_idle_hours: None,
            memory_extract_model: None,
            memory_consolidation_model: None,
            connector_deny_policy,
            approval_handler: None,
        }
    }
}

fn log_codex_backend_request(inhouse_polar_id: &str, method: &str, payload: &Value) {
    if !backend_request_debug_enabled() {
        return;
    }
    let payload_text =
        serde_json::to_string_pretty(payload).unwrap_or_else(|_| payload.to_string());
    debug!(
        event = "backend_request_ready",
        backend = "codex",
        method,
        inhouse_polar_id = %inhouse_polar_id,
        payload = %safe_debug_text(&payload_text),
    );
}

fn log_in_process_codex_turn_completed(
    inhouse_polar_id: &str,
    thread_id: &str,
    turn_id: &str,
    started_at: Instant,
    model: Option<&str>,
    effort: Option<&str>,
    reply: &str,
) {
    info!(
        event = "in_process_codex_turn_completed",
        backend = "codex",
        inhouse_polar_id = %inhouse_polar_id,
        thread_id,
        turn_id,
        model,
        effort,
        duration_ms = started_at.elapsed().as_millis(),
        reply_chars = reply.chars().count(),
        "completed in-process codex turn"
    );
}

fn log_in_process_codex_turn_failed(
    inhouse_polar_id: &str,
    thread_id: &str,
    turn_id: Option<&str>,
    started_at: Instant,
    model: Option<&str>,
    effort: Option<&str>,
    error: &str,
) {
    warn!(
        event = "in_process_codex_turn_failed",
        backend = "codex",
        inhouse_polar_id = %inhouse_polar_id,
        thread_id,
        turn_id = turn_id.unwrap_or(""),
        model,
        effort,
        duration_ms = started_at.elapsed().as_millis(),
        error,
        "failed in-process codex turn"
    );
}

pub struct CodexAgentRunner {
    inner: CodexSessionRunner,
}

pub(crate) fn validate_codex_mode_config(cfg: &CodexAgentConfig) -> anyhow::Result<()> {
    if !cfg.codex_mode.is_conversation() {
        return Ok(());
    }
    anyhow::ensure!(
        cfg.backend == CodexRuntimeBackend::InProcess,
        "codex conversation mode requires runtime_backend=in_process"
    );
    anyhow::ensure!(
        cfg.model
            .as_deref()
            .is_some_and(|model| !model.trim().is_empty()),
        "codex conversation mode requires an explicit fixed model"
    );
    anyhow::ensure!(
        !cfg.memory_tool_enabled
            && cfg.generate_memories.is_none()
            && cfg.use_memories.is_none()
            && cfg.min_rollout_idle_hours.is_none()
            && cfg.memory_extract_model.is_none()
            && cfg.memory_consolidation_model.is_none(),
        "codex conversation mode disables Codex memory configuration"
    );
    Ok(())
}

fn validate_conversation_turn_selection(
    inner: &CodexSessionRunner,
    turn_options: &InhouseTurnOptions,
) -> anyhow::Result<()> {
    let cfg = &inner.cfg;
    if !cfg.codex_mode.is_conversation() {
        return Ok(());
    }
    let fixed_model = cfg
        .model
        .as_deref()
        .expect("conversation mode model validated");
    if let Some(requested_model) = turn_options.model.as_deref() {
        anyhow::ensure!(
            requested_model.trim() == fixed_model.trim(),
            "codex conversation mode model is fixed to '{fixed_model}'"
        );
    }
    if let (Some(fixed_effort), Some(requested_effort)) =
        (cfg.effort.as_deref(), turn_options.effort.as_deref())
    {
        anyhow::ensure!(
            requested_effort.trim() == fixed_effort.trim(),
            "codex conversation mode effort is fixed to '{fixed_effort}'"
        );
    }
    Ok(())
}

struct CodexSessionRunner {
    cfg: CodexAgentConfig,
    client: Option<crate::shared_client::ThreadClient>,
    runtime: Option<Arc<crate::profile_runtime::ProfileRuntime>>,
}

enum TurnPoll {
    Event(Box<AppServerEvent>),
    Return(String),
    Retry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedAppServerError {
    message: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct CodexAgentMessageAccumulator {
    text: String,
}

impl CodexAgentMessageAccumulator {
    fn text(&self) -> &str {
        &self.text
    }

    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn append_delta(
        &mut self,
        delta: &str,
        event_handler: &mut Option<Box<dyn InhouseTurnEventHandler + Send>>,
    ) -> anyhow::Result<()> {
        if delta.is_empty() {
            return Ok(());
        }
        self.text.push_str(delta);
        if let Some(handler) = event_handler.as_deref_mut() {
            handler.on_agent_text_delta(delta)?;
        }
        Ok(())
    }

    fn capture_completed_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InProcessTurnLoopGuardViolation {
    guard_kind: &'static str,
    plan_update_count: usize,
    repeated_signature_count: usize,
    tool_name: Option<String>,
    result_summary: Option<String>,
}

impl std::fmt::Display for InProcessTurnLoopGuardViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.guard_kind {
            "repeated_plan_signature" => write!(
                f,
                "codex in-process turn repeated the same plan update {} times; interrupted possible plan/tool loop",
                self.repeated_signature_count
            ),
            "plan_update_total_limit" => write!(
                f,
                "codex in-process turn exceeded {IN_PROCESS_PLAN_UPDATE_TOTAL_LIMIT} plan updates; interrupted possible plan/tool loop"
            ),
            "repeated_tool_signature" => write!(
                f,
                "codex in-process turn repeated dynamic tool {} with the same arguments and result {} times; last_result={}; interrupted possible tool loop",
                self.tool_name.as_deref().unwrap_or("unknown"),
                self.repeated_signature_count,
                self.result_summary.as_deref().unwrap_or("unknown")
            ),
            "repeated_builtin_function_signature" => write!(
                f,
                "codex in-process turn repeated built-in function {} with the same arguments {} times; interrupted possible unattended tool loop",
                self.tool_name.as_deref().unwrap_or("unknown"),
                self.repeated_signature_count
            ),
            _ => write!(
                f,
                "codex in-process turn violated loop guard {}; interrupted possible plan/tool loop",
                self.guard_kind
            ),
        }
    }
}

#[derive(Default)]
struct InProcessTurnLoopGuard {
    plan_update_count: usize,
    plan_signature_counts: HashMap<String, usize>,
    last_tool_signature: Option<String>,
    repeated_tool_signature_count: usize,
    last_builtin_function_signature: Option<String>,
    repeated_builtin_function_signature_count: usize,
}

impl InProcessTurnLoopGuard {
    fn note_agent_message(&mut self) {
        // Streamed text is UI-visible progress, not terminal turn progress.
    }

    fn note_plan_update(
        &mut self,
        explanation: Option<&str>,
        plan: &[TurnPlanStep],
    ) -> Result<(), InProcessTurnLoopGuardViolation> {
        self.plan_update_count += 1;
        let signature = normalized_plan_update_signature(explanation, plan);
        let repeated_signature_count = {
            let count = self.plan_signature_counts.entry(signature).or_insert(0);
            *count += 1;
            *count
        };

        if repeated_signature_count > IN_PROCESS_REPEATED_PLAN_SIGNATURE_LIMIT {
            return Err(InProcessTurnLoopGuardViolation {
                guard_kind: "repeated_plan_signature",
                plan_update_count: self.plan_update_count,
                repeated_signature_count,
                tool_name: None,
                result_summary: None,
            });
        }

        if self.plan_update_count > IN_PROCESS_PLAN_UPDATE_TOTAL_LIMIT {
            return Err(InProcessTurnLoopGuardViolation {
                guard_kind: "plan_update_total_limit",
                plan_update_count: self.plan_update_count,
                repeated_signature_count,
                tool_name: None,
                result_summary: None,
            });
        }

        Ok(())
    }

    fn note_tool_observation(
        &mut self,
        tool: &str,
        arguments: &Value,
        result: &str,
    ) -> Result<(), InProcessTurnLoopGuardViolation> {
        let signature = normalized_tool_observation_signature(tool, arguments, result);
        if self.last_tool_signature.as_deref() == Some(signature.as_str()) {
            self.repeated_tool_signature_count += 1;
        } else {
            self.last_tool_signature = Some(signature);
            self.repeated_tool_signature_count = 1;
        }
        if self.repeated_tool_signature_count > IN_PROCESS_REPEATED_TOOL_SIGNATURE_LIMIT {
            return Err(InProcessTurnLoopGuardViolation {
                guard_kind: "repeated_tool_signature",
                plan_update_count: self.plan_update_count,
                repeated_signature_count: self.repeated_tool_signature_count,
                tool_name: Some(tool.to_string()),
                result_summary: Some(normalized_tool_result_summary(result)),
            });
        }
        Ok(())
    }

    fn note_builtin_function_call(
        &mut self,
        name: &str,
        arguments: &str,
    ) -> Result<(), InProcessTurnLoopGuardViolation> {
        if name != "request_user_input" {
            self.last_builtin_function_signature = None;
            self.repeated_builtin_function_signature_count = 0;
            return Ok(());
        }
        let signature = normalized_builtin_function_signature(name, arguments);
        if self.last_builtin_function_signature.as_deref() == Some(signature.as_str()) {
            self.repeated_builtin_function_signature_count += 1;
        } else {
            self.last_builtin_function_signature = Some(signature);
            self.repeated_builtin_function_signature_count = 1;
        }
        if self.repeated_builtin_function_signature_count
            > IN_PROCESS_REPEATED_BUILTIN_FUNCTION_SIGNATURE_LIMIT
        {
            return Err(InProcessTurnLoopGuardViolation {
                guard_kind: "repeated_builtin_function_signature",
                plan_update_count: self.plan_update_count,
                repeated_signature_count: self.repeated_builtin_function_signature_count,
                tool_name: Some(name.to_string()),
                result_summary: None,
            });
        }
        Ok(())
    }

    fn note_raw_response_item(
        &mut self,
        item: &ResponseItem,
    ) -> Result<(), InProcessTurnLoopGuardViolation> {
        match item {
            ResponseItem::FunctionCall {
                name, arguments, ..
            } => self.note_builtin_function_call(name, arguments),
            _ => Ok(()),
        }
    }
}

fn normalized_plan_update_signature(explanation: Option<&str>, plan: &[TurnPlanStep]) -> String {
    let mut signature = String::from("explanation:");
    signature.push_str(&normalized_signature_text(explanation.unwrap_or("")));
    for step in plan {
        signature.push('\n');
        signature.push_str(turn_plan_step_status_name(step.status));
        signature.push('|');
        signature.push_str(&normalized_signature_text(&step.step));
    }
    signature
}

fn normalized_tool_observation_signature(tool: &str, arguments: &Value, result: &str) -> String {
    let normalized_result = serde_json::from_str::<Value>(result)
        .ok()
        .and_then(|value| serde_json::to_string(&value).ok())
        .unwrap_or_else(|| normalized_signature_text(result));
    format!(
        "{}\n{}\n{}",
        tool.trim(),
        serde_json::to_string(arguments).unwrap_or_else(|_| arguments.to_string()),
        normalized_result
    )
}

fn normalized_builtin_function_signature(name: &str, arguments: &str) -> String {
    let normalized_arguments = serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|value| serde_json::to_string(&value).ok())
        .unwrap_or_else(|| normalized_signature_text(arguments));
    format!("{}\n{}", name.trim(), normalized_arguments)
}

fn normalized_tool_result_summary(result: &str) -> String {
    let normalized = normalized_signature_text(result);
    normalized.chars().take(240).collect()
}

struct DynamicToolObservation {
    tool: String,
    arguments: Value,
    result: String,
}

fn normalized_signature_text(text: &str) -> String {
    let mut normalized = String::new();
    for part in text.split_whitespace() {
        if !normalized.is_empty() {
            normalized.push(' ');
        }
        normalized.push_str(part);
    }
    normalized
}

fn turn_plan_step_status_name(status: TurnPlanStepStatus) -> &'static str {
    match status {
        TurnPlanStepStatus::Pending => "pending",
        TurnPlanStepStatus::InProgress => "in_progress",
        TurnPlanStepStatus::Completed => "completed",
    }
}

fn raw_response_item_message_text(item: &ResponseItem) -> Option<String> {
    match item {
        ResponseItem::Message { content, .. } => {
            let text = content
                .iter()
                .filter_map(|part| match part {
                    ContentItem::OutputText { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>();
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        }
        _ => None,
    }
}

#[derive(Default)]
struct ReasoningSummaryTracker {
    summary_parts_by_item: HashMap<String, BTreeMap<i64, String>>,
    raw_parts_by_item: HashMap<String, BTreeMap<i64, String>>,
}

impl CodexAgentRunner {
    pub(crate) fn new_managed(
        cfg: CodexAgentConfig,
        runtime: Arc<crate::profile_runtime::ProfileRuntime>,
    ) -> anyhow::Result<Self> {
        if cfg.backend == CodexRuntimeBackend::UnixSocket {
            validate_codex_mode_config(&cfg)?;
            crate::connector_policy::validate_socket_connector_policy(&cfg)?;
        }
        let mut inner = CodexSessionRunner::new(cfg);
        inner.runtime = Some(runtime);
        Ok(Self { inner })
    }

    async fn prepare_managed_config(&mut self) -> anyhow::Result<()> {
        let inner = &mut self.inner;
        if inner.cfg.profile.is_some() {
            let mut resolved = inner.cfg.clone();
            bootstrap_adapter::resolve_native_profile_settings(&mut resolved).await?;
            anyhow::ensure!(
                !inner.cfg.codex_mode.is_conversation()
                    || inner.client.is_none()
                    || resolved.model == inner.cfg.model,
                "Changing the conversation model requires a drained profile runtime restart"
            );
            validate_codex_mode_config(&resolved)?;
            inner.cfg = resolved;
        }
        Ok(())
    }

    pub fn new(cfg: CodexAgentConfig) -> anyhow::Result<Self> {
        validate_codex_mode_config(&cfg)?;
        if cfg.backend == CodexRuntimeBackend::UnixSocket {
            anyhow::ensure!(
                cfg.socket_path
                    .as_deref()
                    .is_some_and(std::path::Path::is_absolute),
                "unix_socket requires an absolute socket_path"
            );
            crate::connector_policy::validate_socket_connector_policy(&cfg)?;
        }
        let inner = CodexSessionRunner::new(cfg);
        Ok(Self { inner })
    }
}

#[async_trait::async_trait]
impl InhouseAgentRunner for CodexAgentRunner {
    async fn discover_model_catalog(
        &mut self,
    ) -> anyhow::Result<Option<InhouseBackendModelCatalog>> {
        self.prepare_managed_config().await?;
        let cfg = &self.inner.cfg;
        if cfg.codex_mode.is_conversation() {
            let model = cfg
                .model
                .as_deref()
                .expect("conversation mode model validated")
                .to_string();
            return Ok(Some(InhouseBackendModelCatalog {
                models: vec![InhouseBackendModel {
                    id: model,
                    is_default: true,
                    thinking_levels: cfg.effort.clone().into_iter().collect(),
                    default_thinking_level: cfg.effort.clone(),
                }],
                provider: cfg.model_provider.clone(),
                source: "configured_fixed".to_string(),
                endpoint: None,
            }));
        }
        let mut cursor = None;
        let mut models = Vec::new();
        loop {
            let (page, next_cursor) = {
                let runner = &mut self.inner;
                runner.ensure_client().await?;
                let request_id = runner.next_request_id();
                let response: ModelListResponse = runner
                    .client_mut()?
                    .request_typed(ClientRequest::ModelList {
                        request_id,
                        params: ModelListParams {
                            model_provider: None,
                            cursor: cursor.clone(),
                            limit: None,
                            include_hidden: Some(false),
                        },
                    })
                    .await
                    .context("model/list for backend model discovery")?;
                let next_cursor = response.next_cursor.clone();
                (
                    model_catalog_from_app_server_value(
                        serde_json::to_value(response)
                            .context("serialize Codex app-server model/list response")?,
                    )?,
                    next_cursor,
                )
            };
            models.extend(page.models);
            let Some(next_cursor) = next_cursor.filter(|value| !value.trim().is_empty()) else {
                break;
            };
            cursor = Some(next_cursor);
        }
        let mut seen = BTreeSet::new();
        models.retain(|model| seen.insert(model.id.clone()));
        Ok(Some(InhouseBackendModelCatalog {
            models,
            provider: None,
            source: "codex_app_server".to_string(),
            endpoint: Some("model/list".to_string()),
        }))
    }

    async fn inspect_session(
        &mut self,
        session_id: &str,
        include_turns: bool,
    ) -> anyhow::Result<Option<Value>> {
        let runner = &mut self.inner;
        runner.ensure_client().await?;
        let request_id = runner.next_request_id();
        let response: ThreadReadResponse = runner
            .client_mut()?
            .request_typed(ClientRequest::ThreadRead {
                request_id,
                params: ThreadReadParams {
                    thread_id: session_id.to_string(),
                    include_turns,
                },
            })
            .await
            .context("thread/read for operator inspection")?;
        Ok(Some(serde_json::to_value(response)?))
    }

    async fn run_turn(
        &mut self,
        existing_session_id: Option<String>,
        input: String,
        turn_options: InhouseTurnOptions,
        dynamic_tool_handler: Option<Arc<dyn InhouseDynamicToolHandler>>,
    ) -> anyhow::Result<(String, String)> {
        ensure_shared_backend_turn_request(&turn_options)?;
        self.prepare_managed_config().await?;
        validate_conversation_turn_selection(&self.inner, &turn_options)?;
        let runner = &mut self.inner;
        runner
            .run_turn(
                existing_session_id,
                input,
                turn_options,
                dynamic_tool_handler,
            )
            .await
    }

    async fn run_turn_streaming(
        &mut self,
        existing_session_id: Option<String>,
        input: String,
        turn_options: InhouseTurnOptions,
        event_handler: Option<Box<dyn InhouseTurnEventHandler + Send>>,
        turn_control: Option<Arc<InhouseTurnControl>>,
        dynamic_tool_handler: Option<Arc<dyn InhouseDynamicToolHandler>>,
    ) -> anyhow::Result<(String, String)> {
        ensure_shared_backend_turn_request(&turn_options)?;
        self.prepare_managed_config().await?;
        validate_conversation_turn_selection(&self.inner, &turn_options)?;
        let runner = &mut self.inner;
        runner
            .run_turn_streaming(
                existing_session_id,
                input,
                turn_options,
                event_handler,
                turn_control,
                dynamic_tool_handler,
            )
            .await
    }
}

impl CodexSessionRunner {
    fn new(cfg: CodexAgentConfig) -> Self {
        Self {
            cfg,
            client: None,
            runtime: None,
        }
    }

    async fn run_turn(
        &mut self,
        existing_session_id: Option<String>,
        input: String,
        turn_options: InhouseTurnOptions,
        dynamic_tool_handler: Option<Arc<dyn InhouseDynamicToolHandler>>,
    ) -> anyhow::Result<(String, String)> {
        self.run_turn_inner(
            existing_session_id,
            input,
            turn_options,
            None,
            None,
            dynamic_tool_handler,
        )
        .await
    }

    async fn run_turn_streaming(
        &mut self,
        existing_session_id: Option<String>,
        input: String,
        turn_options: InhouseTurnOptions,
        event_handler: Option<Box<dyn InhouseTurnEventHandler + Send>>,
        turn_control: Option<Arc<InhouseTurnControl>>,
        dynamic_tool_handler: Option<Arc<dyn InhouseDynamicToolHandler>>,
    ) -> anyhow::Result<(String, String)> {
        self.run_turn_inner(
            existing_session_id,
            input,
            turn_options,
            event_handler,
            turn_control,
            dynamic_tool_handler,
        )
        .await
    }

    async fn run_turn_inner(
        &mut self,
        existing_session_id: Option<String>,
        input: String,
        turn_options: InhouseTurnOptions,
        event_handler: Option<Box<dyn InhouseTurnEventHandler + Send>>,
        turn_control: Option<Arc<InhouseTurnControl>>,
        dynamic_tool_handler: Option<Arc<dyn InhouseDynamicToolHandler>>,
    ) -> anyhow::Result<(String, String)> {
        self.ensure_client().await?;
        let mut lease = None;
        if let Some(id) = &existing_session_id {
            self.client_mut()?.bind_thread(id)?;
            lease = Some(self.client_mut()?.turn_lease()?);
        }
        let (session_id, include_canonical_history) = match existing_session_id {
            Some(id) => {
                self.resume_thread(&id, &turn_options, dynamic_tool_handler.as_ref())
                    .await?;
                (id, false)
            }
            None => (
                self.run_turn_thread(&turn_options, dynamic_tool_handler.as_ref())
                    .await?,
                true,
            ),
        };
        if lease.is_none() {
            self.client_mut()?.bind_thread(&session_id)?;
            lease = Some(self.client_mut()?.turn_lease()?);
        }
        let reply = self
            .start_turn_and_collect(
                &session_id,
                input,
                CodexTurnPromptContext {
                    turn_options: &turn_options,
                    include_canonical_history,
                },
                event_handler,
                turn_control,
                dynamic_tool_handler.as_ref(),
            )
            .await
            .context(crate::thread_lifecycle::LifecycleFailure::Dispatch);
        if let Some(client) = &mut self.client {
            client.finish_turn();
        }
        drop(lease);
        Ok((session_id, reply?))
    }

    async fn ensure_client(&mut self) -> anyhow::Result<()> {
        if self
            .client
            .as_ref()
            .is_some_and(|client| !client.is_closed())
        {
            return Ok(());
        }
        if self.cfg.profile.is_some() {
            bootstrap_adapter::resolve_native_profile_settings(&mut self.cfg).await?;
            validate_codex_mode_config(&self.cfg)?;
        }
        self.client = Some(match &self.runtime {
            Some(runtime) => runtime.lease(&self.cfg).await?,
            None => crate::shared_client::SharedCodexClient::start(match self.cfg.backend {
                CodexRuntimeBackend::InProcess => crate::client::start_embedded(&self.cfg).await?,
                CodexRuntimeBackend::UnixSocket => {
                    crate::client::connect_unix(
                        self.cfg
                            .socket_path
                            .as_deref()
                            .context("missing socket_path")?,
                        "polar-inhouse-agent",
                        Vec::new(),
                    )
                    .await?
                }
            })
            .lease(),
        });
        Ok(())
    }

    async fn run_turn_thread(
        &mut self,
        turn_options: &InhouseTurnOptions,
        dynamic_tool_handler: Option<&Arc<dyn InhouseDynamicToolHandler>>,
    ) -> anyhow::Result<String> {
        let prompt = resolve_codex_prompt_projection(turn_options);
        // Set model_provider only when OSS mode is enabled (e.g., "lmstudio")
        let model_provider = effective_model_provider(
            self.cfg.model_provider.as_deref(),
            self.cfg.oss_mode.as_deref(),
        );
        let request_config = finalize_thread_request_config_overrides(
            self.cfg.codex_mode,
            build_thread_request_config_overrides(
                model_provider.as_deref(),
                effective_provider_base_url(
                    self.cfg.provider_base_url.as_deref(),
                    self.cfg.lmstudio_host.as_deref(),
                )
                .as_deref(),
                prompt.user_instructions.as_deref(),
                memory_overrides_from_config(&self.cfg),
            ),
            dynamic_tool_handler.and_then(|handler| handler.thread_config_overrides()),
        );
        let params = thread_start_params(ThreadStartInput {
            model: turn_options.model.clone().or(self.cfg.model.clone()),
            model_provider,
            cwd: self
                .cfg
                .cwd
                .as_ref()
                .map(|cwd| cwd.to_string_lossy().to_string()),
            approval_policy: parse_approval_policy(self.cfg.approval_policy.as_deref())?,
            sandbox: sandbox_mode_from_policy(self.cfg.sandbox_policy.as_ref())?,
            config: request_config,
            base_instructions: prompt.base_instructions,
            developer_instructions: prompt.developer_instructions,
            dynamic_tools: dynamic_tool_handler.map(|handler| handler.specs()),
        });
        log_codex_backend_request(
            &self.cfg.inhouse_polar_id,
            "thread/start",
            &serde_json::to_value(&params).context("serialize codex thread/start params")?,
        );
        let params_evidence = serde_json::to_value(&params)
            .context("serialize codex thread/start params for provider gate")?;
        prepare_codex_provider_attempt(turn_options, "thread.start", &params_evidence).await?;
        let request_id = self.next_request_id();
        let response: ThreadStartResponse = self
            .client_mut()?
            .request_typed(ClientRequest::ThreadStart { request_id, params })
            .await
            .context("thread/start")?;
        self.client_mut()?.setups().remember(
            &response.thread.id,
            ThreadSetup::from_params(&params_evidence),
        );
        Ok(response.thread.id)
    }

    async fn resume_thread(
        &mut self,
        thread_id: &str,
        turn_options: &InhouseTurnOptions,
        dynamic_tool_handler: Option<&Arc<dyn InhouseDynamicToolHandler>>,
    ) -> anyhow::Result<()> {
        let prompt = resolve_codex_prompt_projection(turn_options);
        // Set model_provider only when OSS mode is enabled (e.g., "lmstudio")
        let model_provider = effective_model_provider(
            self.cfg.model_provider.as_deref(),
            self.cfg.oss_mode.as_deref(),
        );
        let request_config = finalize_thread_request_config_overrides(
            self.cfg.codex_mode,
            build_thread_request_config_overrides(
                model_provider.as_deref(),
                effective_provider_base_url(
                    self.cfg.provider_base_url.as_deref(),
                    self.cfg.lmstudio_host.as_deref(),
                )
                .as_deref(),
                prompt.user_instructions.as_deref(),
                memory_overrides_from_config(&self.cfg),
            ),
            dynamic_tool_handler.and_then(|handler| handler.thread_config_overrides()),
        );
        let params = thread_resume_params(ThreadResumeInput {
            thread_id: thread_id.to_string(),
            model: turn_options.model.clone().or(self.cfg.model.clone()),
            model_provider,
            cwd: self
                .cfg
                .cwd
                .as_ref()
                .map(|cwd| cwd.to_string_lossy().to_string()),
            approval_policy: parse_approval_policy(self.cfg.approval_policy.as_deref())?,
            sandbox: sandbox_mode_from_policy(self.cfg.sandbox_policy.as_ref())?,
            config: request_config,
            base_instructions: prompt.base_instructions,
            developer_instructions: prompt.developer_instructions,
        });
        let params =
            serde_json::to_value(params).context("serialize codex thread/resume params")?;
        prepare_existing(self.client_mut()?, thread_id, params, turn_options).await
    }

    async fn start_turn_and_collect(
        &mut self,
        thread_id: &str,
        input: String,
        prompt_context: CodexTurnPromptContext<'_>,
        mut event_handler: Option<Box<dyn InhouseTurnEventHandler + Send>>,
        turn_control: Option<Arc<InhouseTurnControl>>,
        dynamic_tool_handler: Option<&Arc<dyn InhouseDynamicToolHandler>>,
    ) -> anyhow::Result<String> {
        self.client_mut()?.bind_thread(thread_id)?;
        let turn_options = prompt_context.turn_options;
        let prompt = resolve_codex_prompt_projection(turn_options);
        let turn_prompt =
            codex_turn_prompt_prefix(&prompt, prompt_context.include_canonical_history);
        let effective_input = prepend_turn_prompt_to_input(&input, turn_prompt.as_deref());
        let prepared = build_user_inputs(&effective_input, self.cfg.cwd.as_ref()).await?;
        let model = turn_options
            .model
            .clone()
            .or_else(|| self.cfg.model.clone());
        let effort = turn_options
            .effort
            .clone()
            .or_else(|| self.cfg.effort.clone());
        info!(
            inhouse_polar_id = %self.cfg.inhouse_polar_id,
            thread_id,
            cwd = ?self.cfg.cwd,
            prepared_input_items = prepared.len(),
            model = model.as_deref(),
            effort = effort.as_deref(),
            approval_policy = ?self.cfg.approval_policy,
            "starting in-process codex turn"
        );
        let params = turn_start_params(TurnStartInput {
            thread_id: thread_id.to_string(),
            input: prepared,
            cwd: self.cfg.cwd.clone(),
            approval_policy: parse_approval_policy(self.cfg.approval_policy.as_deref())?,
            sandbox_policy: parse_sandbox_policy(self.cfg.sandbox_policy.clone())?,
            model: model.clone(),
            effort: parse_reasoning_effort(effort.as_deref())?,
        });
        log_codex_backend_request(
            &self.cfg.inhouse_polar_id,
            "turn/start",
            &serde_json::to_value(&params).context("serialize codex turn/start params")?,
        );
        let params_evidence = serde_json::to_value(&params)
            .context("serialize codex turn/start params for provider gate")?;
        prepare_codex_provider_attempt(turn_options, "turn.dispatch", &params_evidence).await?;
        let turn_started_at = Instant::now();
        let request_id = self.next_request_id();
        let TurnStartResponse { turn } = match self
            .client_mut()?
            .request_typed(ClientRequest::TurnStart { request_id, params })
            .await
        {
            Ok(response) => response,
            Err(error) => {
                let error = turn_start_request_error(error);
                log_in_process_codex_turn_failed(
                    &self.cfg.inhouse_polar_id,
                    thread_id,
                    None,
                    turn_started_at,
                    model.as_deref(),
                    effort.as_deref(),
                    &format!("{error:#}"),
                );
                return Err(error);
            }
        };
        let turn_id = turn.id;

        let mut agent_message = CodexAgentMessageAccumulator::default();
        let mut agent_message_id: Option<String> = None;
        let mut app_server_error: Option<CapturedAppServerError> = None;
        let mut reasoning_summary = ReasoningSummaryTracker::default();
        let mut turn_loop_guard = InProcessTurnLoopGuard::default();
        let mut interrupt_sent = false;

        loop {
            let event = if !interrupt_sent {
                if let Some(control) = turn_control.as_ref() {
                    tokio::select! {
                        _ = control.wait_for_stop() => {
                            let request_id = self.next_request_id();
                            let _: TurnInterruptResponse = self
                                .client_mut()?
                                .request_typed(
                                    ClientRequest::TurnInterrupt {
                                        request_id,
                                        params: TurnInterruptParams {
                                            thread_id: thread_id.to_string(),
                                            turn_id: turn_id.clone(),
                                        },
                                    }
                                )
                                .await
                                .context("turn/interrupt")?;
                            interrupt_sent = true;
                            continue;
                        }
                        poll = self.poll_next_in_process_turn_event(thread_id, &turn_id, agent_message.text(), app_server_error.as_ref()) => {
                            match poll? {
                                TurnPoll::Event(event) => *event,
                                TurnPoll::Return(reply) => {
                                    log_in_process_codex_turn_completed(
                                        &self.cfg.inhouse_polar_id,
                                        thread_id,
                                        &turn_id,
                                        turn_started_at,
                                        model.as_deref(),
                                        effort.as_deref(),
                                        &reply,
                                    );
                                    return Ok(reply);
                                }
                                TurnPoll::Retry => continue,
                            }
                        }
                    }
                } else {
                    match self
                        .poll_next_in_process_turn_event(
                            thread_id,
                            &turn_id,
                            agent_message.text(),
                            app_server_error.as_ref(),
                        )
                        .await?
                    {
                        TurnPoll::Event(event) => *event,
                        TurnPoll::Return(reply) => {
                            log_in_process_codex_turn_completed(
                                &self.cfg.inhouse_polar_id,
                                thread_id,
                                &turn_id,
                                turn_started_at,
                                model.as_deref(),
                                effort.as_deref(),
                                &reply,
                            );
                            return Ok(reply);
                        }
                        TurnPoll::Retry => continue,
                    }
                }
            } else {
                match self
                    .poll_next_in_process_turn_event(
                        thread_id,
                        &turn_id,
                        agent_message.text(),
                        app_server_error.as_ref(),
                    )
                    .await?
                {
                    TurnPoll::Event(event) => *event,
                    TurnPoll::Return(reply) => {
                        log_in_process_codex_turn_completed(
                            &self.cfg.inhouse_polar_id,
                            thread_id,
                            &turn_id,
                            turn_started_at,
                            model.as_deref(),
                            effort.as_deref(),
                            &reply,
                        );
                        return Ok(reply);
                    }
                    TurnPoll::Retry => continue,
                }
            };
            match event {
                AppServerEvent::ServerRequest(request) => {
                    let observation = self
                        .handle_in_process_server_request(
                            *request,
                            dynamic_tool_handler,
                            &mut event_handler,
                        )
                        .await?;
                    if let Some(observation) = observation {
                        if let Err(violation) = turn_loop_guard.note_tool_observation(
                            &observation.tool,
                            &observation.arguments,
                            &observation.result,
                        ) {
                            let error_text = violation.to_string();
                            warn!(
                                thread_id,
                                turn_id = %turn_id,
                                guard_kind = violation.guard_kind,
                                repeated_signature_count = violation.repeated_signature_count,
                                tool = %observation.tool,
                                error = %error_text,
                                "interrupting codex in-process turn after repeated dynamic-tool loop"
                            );
                            if let Err(interrupt_error) =
                                self.interrupt_in_process_turn(thread_id, &turn_id).await
                            {
                                warn!(
                                    thread_id,
                                    turn_id = %turn_id,
                                    error = %format!("{interrupt_error:#}"),
                                    "failed to interrupt codex in-process turn after dynamic-tool loop"
                                );
                            }
                            log_in_process_codex_turn_failed(
                                &self.cfg.inhouse_polar_id,
                                thread_id,
                                Some(&turn_id),
                                turn_started_at,
                                model.as_deref(),
                                effort.as_deref(),
                                &error_text,
                            );
                            return Err(anyhow::anyhow!(error_text));
                        }
                    }
                }
                AppServerEvent::ServerNotification(notification) => match *notification {
                    ServerNotification::ItemStarted(params) => {
                        capture_agent_message_started(&params, &mut agent_message_id);
                        if let Some(handler) = event_handler.as_deref_mut() {
                            observe_native_tool(handler, &params.item, false).await?;
                            if let Some((phase, text)) = item_started_status(&params.item) {
                                if phase != "tool" { handler.on_intermediate_status(&text, phase)?; }
                            }
                        }
                    }
                    ServerNotification::AgentMessageDelta(params)
                        if agent_message_id.is_none()
                            || agent_message_id.as_deref() == Some(params.item_id.as_str()) =>
                    {
                        agent_message.append_delta(&params.delta, &mut event_handler)?;
                        turn_loop_guard.note_agent_message();
                    }
                    ServerNotification::ItemCompleted(params) => {
                        capture_agent_message_completed(&params, &mut agent_message);
                        if !agent_message.is_empty() {
                            turn_loop_guard.note_agent_message();
                        }
                        if let Some(handler) = event_handler.as_deref_mut() {
                            observe_native_tool(handler, &params.item, true).await?;
                            if let Some((phase, text)) = item_completed_status(&params.item) {
                                if phase != "tool" { handler.on_intermediate_status(&text, phase)?; }
                            }
                        }
                        reasoning_summary.remove_completed_item(&params.item);
                    }
                    ServerNotification::RawResponseItemCompleted(params) => {
                        if params.thread_id != thread_id {
                            continue;
                        }
                        if let Err(violation) = turn_loop_guard.note_raw_response_item(&params.item)
                        {
                            let tool_name = violation
                                .tool_name
                                .as_deref()
                                .unwrap_or("unknown")
                                .to_string();
                            let error_text = violation.to_string();
                            warn!(
                                thread_id,
                                turn_id = %turn_id,
                                guard_kind = violation.guard_kind,
                                repeated_signature_count = violation.repeated_signature_count,
                                tool = %tool_name,
                                error = %error_text,
                                "interrupting codex in-process turn after repeated built-in function loop"
                            );
                            if let Err(interrupt_error) =
                                self.interrupt_in_process_turn(thread_id, &turn_id).await
                            {
                                warn!(
                                    thread_id,
                                    turn_id = %turn_id,
                                    error = %format!("{interrupt_error:#}"),
                                    "failed to interrupt codex in-process turn after built-in function loop"
                                );
                            }
                            log_in_process_codex_turn_failed(
                                &self.cfg.inhouse_polar_id,
                                thread_id,
                                Some(&turn_id),
                                turn_started_at,
                                model.as_deref(),
                                effort.as_deref(),
                                &error_text,
                            );
                            return Err(anyhow::anyhow!(error_text));
                        }
                        if let Some(text) = raw_response_item_message_text(&params.item) {
                            agent_message.capture_completed_text(text);
                            turn_loop_guard.note_agent_message();
                        }
                    }
                    ServerNotification::TurnCompleted(params) => {
                        if params.thread_id != thread_id {
                            continue;
                        }
                        let resolved = if params.turn.items.is_empty() && agent_message.is_empty() {
                            self.read_turn_from_thread_history(thread_id, &params.turn.id)
                                .await?
                                .unwrap_or_else(|| params.turn.clone())
                        } else {
                            params.turn.clone()
                        };
                        if let Some(reply) = turn_terminal_reply(
                            &resolved,
                            agent_message.text(),
                            app_server_error.as_ref(),
                        )? {
                            if !reply.is_empty() {
                                agent_message.capture_completed_text(reply.clone());
                            }
                            log_in_process_codex_turn_completed(
                                &self.cfg.inhouse_polar_id,
                                thread_id,
                                &turn_id,
                                turn_started_at,
                                model.as_deref(),
                                effort.as_deref(),
                                &reply,
                            );
                            return Ok(reply);
                        }
                    }
                    ServerNotification::TurnPlanUpdated(params) => {
                        if params.thread_id != thread_id {
                            continue;
                        }
                        if let Some(handler) = event_handler.as_deref_mut() {
                            let text =
                                plan_update_text(params.explanation.as_deref(), &params.plan);
                            handler.on_plan_update(&text)?;
                        }
                        if let Err(violation) = turn_loop_guard
                            .note_plan_update(params.explanation.as_deref(), &params.plan)
                        {
                            let error_text = violation.to_string();
                            warn!(
                                thread_id,
                                turn_id = %turn_id,
                                guard_kind = violation.guard_kind,
                                plan_update_count = violation.plan_update_count,
                                repeated_signature_count = violation.repeated_signature_count,
                                agent_chars = agent_message.text().chars().count(),
                                error = %error_text,
                                "interrupting codex in-process turn after plan/tool loop guard violation"
                            );
                            if let Err(interrupt_error) =
                                self.interrupt_in_process_turn(thread_id, &turn_id).await
                            {
                                warn!(
                                    thread_id,
                                    turn_id = %turn_id,
                                    error = %format!("{interrupt_error:#}"),
                                    "failed to interrupt codex in-process turn after plan-update loop"
                                );
                            }
                            log_in_process_codex_turn_failed(
                                &self.cfg.inhouse_polar_id,
                                thread_id,
                                Some(&turn_id),
                                turn_started_at,
                                model.as_deref(),
                                effort.as_deref(),
                                &error_text,
                            );
                            return Err(anyhow::anyhow!(error_text));
                        }
                    }
                    ServerNotification::Error(params) => {
                        if let Some(error) =
                            capture_app_server_error_from_notification(&params, thread_id, &turn_id)
                        {
                            if error.will_retry {
                                app_server_error = Some(error.captured);
                            } else if let Some(model_input_error) = error.model_input_error {
                                let error_text = model_input_error.to_string();
                                log_in_process_codex_turn_failed(
                                    &self.cfg.inhouse_polar_id,
                                    thread_id,
                                    Some(&turn_id),
                                    turn_started_at,
                                    model.as_deref(),
                                    effort.as_deref(),
                                    &error_text,
                                );
                                return Err(model_input_error.into());
                            } else {
                                let error_text = format!("turn failed: {}", error.captured.message);
                                log_in_process_codex_turn_failed(
                                    &self.cfg.inhouse_polar_id,
                                    thread_id,
                                    Some(&turn_id),
                                    turn_started_at,
                                    model.as_deref(),
                                    effort.as_deref(),
                                    &error_text,
                                );
                                return Err(anyhow::anyhow!("{}", error_text));
                            }
                        }
                    }
                    ServerNotification::ReasoningSummaryPartAdded(params) => {
                        if params.thread_id != thread_id {
                            continue;
                        }
                        reasoning_summary.ensure_part(&params.item_id, params.summary_index);
                    }
                    ServerNotification::ReasoningSummaryTextDelta(params) => {
                        if params.thread_id != thread_id {
                            continue;
                        }
                        if let Some(handler) = event_handler.as_deref_mut() {
                            if let Some(text) = reasoning_summary.apply_summary_delta(
                                &params.item_id,
                                params.summary_index,
                                &params.delta,
                            ) {
                                handler.on_intermediate_status(&text, "thinking")?;
                            }
                        }
                    }
                    ServerNotification::ReasoningTextDelta(params) => {
                        if params.thread_id != thread_id {
                            continue;
                        }
                        if let Some(handler) = event_handler.as_deref_mut() {
                            if let Some(text) = reasoning_summary.apply_raw_delta(
                                &params.item_id,
                                params.content_index,
                                &params.delta,
                            ) {
                                handler.on_intermediate_status(&text, "thinking")?;
                            }
                        }
                    }
                    _ => {}
                },
                AppServerEvent::Disconnected { message } => anyhow::bail!("Codex disconnected during turn: {message}"),
                AppServerEvent::ThreadStreamDisconnected(_) | AppServerEvent::ThreadStreamReconnected(_) => continue,
                AppServerEvent::Lagged { skipped } => {
                    if skipped == 0 && !agent_message.is_empty() {
                        return Ok(agent_message.text().to_string());
                    }
                    warn!(skipped, "codex in-process event stream lagged");
                }
            }
        }
    }

    async fn handle_in_process_server_request(
        &mut self,
        request: ServerRequest,
        dynamic_tool_handler: Option<&Arc<dyn InhouseDynamicToolHandler>>,
        event_handler: &mut Option<Box<dyn InhouseTurnEventHandler + Send>>,
    ) -> anyhow::Result<Option<DynamicToolObservation>> {
        match request {
            ServerRequest::CommandExecutionRequestApproval { request_id, params } => {
                let result = serde_json::to_value(CommandExecutionRequestApprovalResponse {
                    decision: self.request_command_approval(&params).await?,
                })?;
                self.client_mut()?
                    .resolve_server_request(request_id, result)
                    .await?;
                Ok(None)
            }
            ServerRequest::FileChangeRequestApproval { request_id, params } => {
                let result = serde_json::to_value(FileChangeRequestApprovalResponse {
                    decision: self.request_file_change_approval(&params).await?,
                })?;
                self.client_mut()?
                    .resolve_server_request(request_id, result)
                    .await?;
                Ok(None)
            }
            ServerRequest::ToolRequestUserInput { request_id, .. } => {
                warn!("codex requested user input without UI; replying empty");
                let result = serde_json::to_value(ToolRequestUserInputResponse {
                    answers: HashMap::new(),
                })?;
                self.client_mut()?
                    .resolve_server_request(request_id, result)
                    .await?;
                Ok(None)
            }
            ServerRequest::DynamicToolCall { request_id, params } => {
                let Some(handler) = dynamic_tool_handler else {
                    let message =
                        "polar inhouse adapter does not support dynamic tools for this turn";
                    self.client_mut()?
                        .reject_server_request(
                            request_id,
                            JSONRPCErrorError {
                                code: -32603,
                                message: message.to_string(),
                                data: None,
                            },
                        )
                        .await?;
                    anyhow::bail!("{message}");
                };
                if let Some(handler) = event_handler.as_deref_mut() {
                    handler.on_tool_started(&InhouseToolCallEnvelope {
                    call_id: params.call_id.clone(), provider_call_id: Some(params.call_id.clone()),
                    tool_name: params.tool.clone(), arguments: params.arguments.clone(),
                }).await?;
                }
                let text = match call_dynamic_tool(
                    handler.as_ref(),
                    &params.tool,
                    params.arguments.clone(),
                )
                .await
                {
                    Ok(text) => text,
                    Err(err) => {
                        let error_text = dynamic_tool_call_error_text(&err);
                        if let Some(handler) = event_handler.as_deref_mut() {
                            handler.on_tool_finished(&InhouseToolResultEnvelope {
                    call_id: params.call_id.clone(), tool_name: params.tool.clone(),
                    arguments: params.arguments.clone(), result_text: error_text.clone(), ok: false,
                    argument_correction_allowed: false, error_kind: None, retry_guidance: None,
                }).await?;
                        }
                        let result = serde_json::to_value(dynamic_tool_call_error_response(&err))?;
                        self.client_mut()?
                            .resolve_server_request(request_id, result)
                            .await?;
                        return Ok(Some(DynamicToolObservation {
                            tool: params.tool,
                            arguments: params.arguments,
                            result: error_text,
                        }));
                    }
                };
                if let Some(handler) = event_handler.as_deref_mut() {
                    handler.on_tool_finished(&InhouseToolResultEnvelope {
                    call_id: params.call_id.clone(), tool_name: params.tool.clone(),
                    arguments: params.arguments.clone(), result_text: text.clone(), ok: true,
                    argument_correction_allowed: false, error_kind: None, retry_guidance: None,
                }).await?;
                }
                let result = serde_json::to_value(DynamicToolCallResponse {
                    content_items: vec![DynamicToolCallOutputContentItem::InputText {
                        text: text.clone(),
                    }],
                    success: true,
                })?;
                self.client_mut()?
                    .resolve_server_request(request_id, result)
                    .await?;
                Ok(Some(DynamicToolObservation {
                    tool: params.tool,
                    arguments: params.arguments,
                    result: text,
                }))
            }
            other => {
                let request_id = other.id().clone();
                warn!(request_id = ?request_id, "unsupported codex server request for in-process polar adapter");
                self.client_mut()?
                    .reject_server_request(
                        request_id,
                        JSONRPCErrorError {
                            code: -32603,
                            message: "polar inhouse adapter does not support this server request"
                                .to_string(),
                            data: None,
                        },
                    )
                    .await?;
                Ok(None)
            }
        }
    }

    async fn interrupt_in_process_turn(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> anyhow::Result<()> {
        let request_id = self.next_request_id();
        let _: TurnInterruptResponse = self
            .client_mut()?
            .request_typed(ClientRequest::TurnInterrupt {
                request_id,
                params: TurnInterruptParams {
                    thread_id: thread_id.to_string(),
                    turn_id: turn_id.to_string(),
                },
            })
            .await
            .context("turn/interrupt")?;
        Ok(())
    }

    async fn poll_next_in_process_turn_event(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        agent_message: &str,
        app_server_error: Option<&CapturedAppServerError>,
    ) -> anyhow::Result<TurnPoll> {
        match tokio::time::timeout(TURN_IDLE_TIMEOUT, self.client_mut()?.next_event()).await {
            Ok(Some(event)) => Ok(TurnPoll::Event(Box::new(event))),
            Ok(None) => Err(anyhow::anyhow!("codex in-process event stream closed")),
            Err(_) => {
                if let Some(turn) = self
                    .read_turn_from_thread_history(thread_id, turn_id)
                    .await?
                {
                    match turn_terminal_reply(&turn, agent_message, app_server_error)? {
                        Some(reply) => {
                            warn!(
                                thread_id,
                                turn_id,
                                "codex in-process turn recovered terminal reply from thread/read after quiet period"
                            );
                            return Ok(TurnPoll::Return(reply));
                        }
                        None => {
                            if !agent_message.is_empty() {
                                warn!(
                                    thread_id,
                                    turn_id,
                                    timeout_secs = TURN_IDLE_TIMEOUT.as_secs(),
                                    agent_message_len = agent_message.len(),
                                    "codex in-process turn is still in progress after quiet period; waiting for more events"
                                );
                            }
                            return Ok(TurnPoll::Retry);
                        }
                    }
                }
                if !agent_message.is_empty() {
                    warn!(
                        thread_id,
                        timeout_secs = TURN_IDLE_TIMEOUT.as_secs(),
                        agent_message_len = agent_message.len(),
                        "codex in-process turn went idle after agent output without thread history; using accumulated agent message"
                    );
                    Ok(TurnPoll::Return(agent_message.to_string()))
                } else {
                    Ok(TurnPoll::Retry)
                }
            }
        }
    }

    async fn read_turn_from_thread_history(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> anyhow::Result<Option<Turn>> {
        let request_id = self.next_request_id();
        let response: ThreadReadResponse = self
            .client_mut()?
            .request_typed(ClientRequest::ThreadRead {
                request_id,
                params: ThreadReadParams {
                    thread_id: thread_id.to_string(),
                    include_turns: true,
                },
            })
            .await
            .context("thread/read")?;
        Ok(turn_from_thread_history(&response.thread.turns, turn_id))
    }

    async fn request_command_approval(
        &self,
        params: &CommandExecutionRequestApprovalParams,
    ) -> anyhow::Result<CommandExecutionApprovalDecision> {
        let Some(handler) = self.cfg.approval_handler.as_ref() else {
            warn!("codex requested command approval without UI; declining");
            return Ok(CommandExecutionApprovalDecision::Decline);
        };
        let decision = handler
            .request_approval(InhouseApprovalRequest {
                approval_key: format!(
                    "cmd:{}:{}:{}",
                    params.thread_id, params.turn_id, params.item_id
                ),
                inhouse_polar_id: self.cfg.inhouse_polar_id.clone(),
                thread_id: params.thread_id.clone(),
                turn_id: params.turn_id.clone(),
                item_id: params.item_id.clone(),
                reason: params.reason.clone(),
                kind: InhouseApprovalKind::CommandExecution {
                    command: params.command.clone(),
                    cwd: command_execution_cwd(params),
                },
            })
            .await?;
        Ok(match decision {
            InhouseApprovalDecision::Accept => CommandExecutionApprovalDecision::Accept,
            InhouseApprovalDecision::AcceptForSession => {
                CommandExecutionApprovalDecision::AcceptForSession
            }
            InhouseApprovalDecision::Decline => CommandExecutionApprovalDecision::Decline,
            InhouseApprovalDecision::Cancel => CommandExecutionApprovalDecision::Cancel,
        })
    }

    async fn request_file_change_approval(
        &self,
        params: &FileChangeRequestApprovalParams,
    ) -> anyhow::Result<FileChangeApprovalDecision> {
        let Some(handler) = self.cfg.approval_handler.as_ref() else {
            warn!("codex requested file-change approval without UI; declining");
            return Ok(FileChangeApprovalDecision::Decline);
        };
        let decision = handler
            .request_approval(InhouseApprovalRequest {
                approval_key: format!(
                    "file:{}:{}:{}",
                    params.thread_id, params.turn_id, params.item_id
                ),
                inhouse_polar_id: self.cfg.inhouse_polar_id.clone(),
                thread_id: params.thread_id.clone(),
                turn_id: params.turn_id.clone(),
                item_id: params.item_id.clone(),
                reason: params.reason.clone(),
                kind: InhouseApprovalKind::FileChange {
                    grant_root: params.grant_root.clone(),
                },
            })
            .await?;
        Ok(match decision {
            InhouseApprovalDecision::Accept => FileChangeApprovalDecision::Accept,
            InhouseApprovalDecision::AcceptForSession => {
                FileChangeApprovalDecision::AcceptForSession
            }
            InhouseApprovalDecision::Decline => FileChangeApprovalDecision::Decline,
            InhouseApprovalDecision::Cancel => FileChangeApprovalDecision::Cancel,
        })
    }

    fn client_mut(&mut self) -> anyhow::Result<&mut crate::shared_client::ThreadClient> {
        self.client
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("codex in-process app-server not running"))
    }

    fn next_request_id(&mut self) -> RequestId {
        static NEXT_ID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        RequestId::Integer(id)
    }
}

fn capture_agent_message_started(
    params: &ItemStartedNotification,
    agent_message_id: &mut Option<String>,
) {
    if let ThreadItemPayload::AgentMessage { id, .. } = &params.item.payload {
        *agent_message_id = Some(id.clone());
    }
}

fn capture_agent_message_completed(
    params: &ItemCompletedNotification,
    agent_message: &mut CodexAgentMessageAccumulator,
) {
    if let ThreadItemPayload::AgentMessage { text, .. } = &params.item.payload {
        agent_message.capture_completed_text(text.clone());
    }
}

async fn call_dynamic_tool(
    handler: &dyn InhouseDynamicToolHandler,
    tool: &str,
    arguments: Value,
) -> anyhow::Result<String> {
    let started = Instant::now();
    info!(
        event = "codex_dynamic_tool_call_started",
        tool,
        argument_bytes = arguments.to_string().len(),
        "codex dynamic tool call started"
    );
    // Execution limits belong to the tool/executor. An adapter deadline can
    // abandon the result while the SDK operation still runs and commits it.
    match handler.handle_call(tool, arguments).await {
        Ok(text) => {
            info!(
                event = "codex_dynamic_tool_call_completed",
                tool,
                duration_ms = started.elapsed().as_millis() as u64,
                result_bytes = text.len(),
                "codex dynamic tool call completed"
            );
            Ok(text)
        }
        Err(err) => {
            error!(
                event = "codex_dynamic_tool_call_failed",
                tool,
                duration_ms = started.elapsed().as_millis() as u64,
                error = %format!("{err:#}"),
                "codex dynamic tool call failed"
            );
            Err(err).with_context(|| format!("dynamic tool call failed: {tool}"))
        }
    }
}

fn dynamic_tool_call_error_text(error: &anyhow::Error) -> String {
    serde_json::json!({
        "error": format!("{error:#}"),
    })
    .to_string()
}

fn dynamic_tool_call_error_response(error: &anyhow::Error) -> DynamicToolCallResponse {
    DynamicToolCallResponse {
        content_items: vec![DynamicToolCallOutputContentItem::InputText {
            text: dynamic_tool_call_error_text(error),
        }],
        success: false,
    }
}

fn turn_from_thread_history(turns: &[Turn], turn_id: &str) -> Option<Turn> {
    turns.iter().find(|turn| turn.id == turn_id).cloned()
}

fn agent_message_from_thread_items(items: &[ThreadItem]) -> Option<String> {
    items.iter().rev().find_map(|item| match &item.payload {
        ThreadItemPayload::AgentMessage { text, .. } if !text.is_empty() => Some(text.clone()),
        _ => None,
    })
}

fn app_server_error_message(error: &TurnError) -> String {
    let message = error.message.trim();
    let details = error
        .additional_details
        .as_deref()
        .map(str::trim)
        .filter(|details| !details.is_empty());
    match (message.is_empty(), details) {
        (true, Some(details)) => details.to_string(),
        (true, None) => "codex app-server reported an error".to_string(),
        (false, Some(details)) => format!("{message}\n\n{details}"),
        (false, None) => message.to_string(),
    }
}

struct AppServerErrorCapture {
    captured: CapturedAppServerError,
    will_retry: bool,
    model_input_error: Option<InhouseModelInputError>,
}

fn model_input_error_from_jsonrpc_error(
    error: &JSONRPCErrorError,
) -> Option<InhouseModelInputError> {
    let data = error.data.as_ref();
    let error_type = data
        .and_then(|data| data.get("type"))
        .or_else(|| {
            data.and_then(|data| data.get("error"))
                .and_then(|err| err.get("type"))
        })
        .and_then(Value::as_str)
        .unwrap_or_default();
    let param = data
        .and_then(|data| data.get("param"))
        .or_else(|| {
            data.and_then(|data| data.get("error"))
                .and_then(|err| err.get("param"))
        })
        .and_then(Value::as_str)
        .unwrap_or_default();
    let nested_message = data
        .and_then(|data| data.get("message"))
        .or_else(|| {
            data.and_then(|data| data.get("error"))
                .and_then(|err| err.get("message"))
        })
        .and_then(Value::as_str);

    let looks_like_input_request_error =
        error_type == "invalid_request_error" && (param.is_empty() || param == "input");
    if looks_like_input_request_error {
        // Build detailed error info for logging
        let mut detail_parts = vec![
            format!("type={}", error_type),
            format!("param={}", if param.is_empty() { "input" } else { param }),
            error.message.clone(),
        ];
        if let Some(msg) = nested_message {
            detail_parts.push(format!("nested_message={}", msg));
        }
        detail_parts.push(format!("code={}", error.code));
        let detail = detail_parts.join("; ");
        return model_input_error_from_text_with_detail(&error.message, nested_message, &detail);
    }
    model_input_error_from_text_with_detail(&error.message, nested_message, &error.message)
}

fn model_input_error_from_text_with_detail(
    message: &str,
    additional_details: Option<&str>,
    detail: &str,
) -> Option<InhouseModelInputError> {
    let combined = [Some(message), additional_details]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n");
    let normalized = combined.to_ascii_lowercase();
    if normalized.contains("image")
        && (normalized.contains("does not support image input")
            || normalized.contains("do not support image input")
            || normalized.contains("doesn't support image input")
            || normalized.contains("not support image inputs")
            || normalized.contains("unsupported image input")
            || normalized.contains("image inputs are not supported"))
    {
        return Some(InhouseModelInputError::with_detail(
            "the model does not support image inputs.",
            detail,
        ));
    }
    None
}

fn model_input_error_from_turn_error(error: &TurnError) -> Option<InhouseModelInputError> {
    let mut detail_parts = vec![error.message.clone()];
    if let Some(details) = &error.additional_details {
        detail_parts.push(format!("additional_details={}", details));
    }
    let detail = detail_parts.join("; ");
    model_input_error_from_text_with_detail(
        &error.message,
        error.additional_details.as_deref(),
        &detail,
    )
}

fn model_input_error_from_typed_request_error(
    error: &TypedRequestError,
) -> Option<InhouseModelInputError> {
    match error {
        TypedRequestError::Server { source, .. } => model_input_error_from_jsonrpc_error(source),
        _ => None,
    }
}

fn turn_start_request_error(error: TypedRequestError) -> anyhow::Error {
    if let Some(model_input_error) = model_input_error_from_typed_request_error(&error) {
        return model_input_error.into();
    }
    anyhow::Error::new(error).context("turn/start")
}

fn capture_app_server_error_from_notification(
    notification: &ErrorNotification,
    thread_id: &str,
    turn_id: &str,
) -> Option<AppServerErrorCapture> {
    if notification.thread_id != thread_id || notification.turn_id != turn_id {
        return None;
    }
    let model_input_error = model_input_error_from_turn_error(&notification.error);
    Some(AppServerErrorCapture {
        captured: CapturedAppServerError {
            message: app_server_error_message(&notification.error),
        },
        will_retry: notification.will_retry,
        model_input_error,
    })
}

fn turn_terminal_reply(
    turn: &Turn,
    current_agent_message: &str,
    app_server_error: Option<&CapturedAppServerError>,
) -> anyhow::Result<Option<String>> {
    match turn.status {
        TurnStatus::Completed => {
            let message = if !current_agent_message.is_empty() {
                Some(current_agent_message.to_string())
            } else {
                agent_message_from_thread_items(&turn.items)
            };
            match message {
                Some(message) => Ok(Some(message)),
                None => {
                    if let Some(error) = app_server_error {
                        Err(anyhow::anyhow!("turn failed: {}", error.message))
                    } else {
                        Err(anyhow::anyhow!("turn completed without agent message"))
                    }
                }
            }
        }
        TurnStatus::Interrupted => Ok(Some(if !current_agent_message.is_empty() {
            current_agent_message.to_string()
        } else {
            agent_message_from_thread_items(&turn.items).unwrap_or_default()
        })),
        TurnStatus::Failed => {
            let message = turn
                .error
                .as_ref()
                .map(|error| error.message.clone())
                .unwrap_or_else(|| "turn failed".to_string());
            Err(anyhow::anyhow!("turn failed: {message}"))
        }
        TurnStatus::InProgress => Ok(None),
    }
}

impl ReasoningSummaryTracker {
    fn ensure_part(&mut self, item_id: &str, summary_index: i64) {
        self.summary_parts_by_item
            .entry(item_id.to_string())
            .or_default()
            .entry(summary_index)
            .or_default();
    }

    fn apply_summary_delta(
        &mut self,
        item_id: &str,
        summary_index: i64,
        delta: &str,
    ) -> Option<String> {
        let parts = self
            .summary_parts_by_item
            .entry(item_id.to_string())
            .or_default();
        parts.entry(summary_index).or_default().push_str(delta);
        self.render_item(item_id)
    }

    fn apply_raw_delta(
        &mut self,
        item_id: &str,
        content_index: i64,
        delta: &str,
    ) -> Option<String> {
        let parts = self
            .raw_parts_by_item
            .entry(item_id.to_string())
            .or_default();
        parts.entry(content_index).or_default().push_str(delta);
        self.render_item(item_id)
    }

    fn remove_completed_item(&mut self, item: &ThreadItem) {
        if let ThreadItemPayload::Reasoning { id, .. } = &item.payload {
            self.summary_parts_by_item.remove(id);
            self.raw_parts_by_item.remove(id);
        }
    }

    fn render_item(&self, item_id: &str) -> Option<String> {
        self.summary_parts_by_item
            .get(item_id)
            .and_then(render_reasoning_summary_parts)
            .or_else(|| {
                self.raw_parts_by_item
                    .get(item_id)
                    .and_then(render_reasoning_summary_parts)
            })
    }
}

fn render_reasoning_summary_parts(parts: &BTreeMap<i64, String>) -> Option<String> {
    let joined = parts
        .values()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

async fn observe_native_tool(handler: &mut dyn InhouseTurnEventHandler, item: &ThreadItem, completed: bool) -> anyhow::Result<()> {
    let (id, name, ok) = match &item.payload {
        ThreadItemPayload::CommandExecution { id, exit_code, .. } => (id, "command_execution".to_owned(), *exit_code == Some(0)),
        ThreadItemPayload::McpToolCall { id, server, tool, error, .. } => (id, format!("{server}/{tool}"), error.is_none()),
        ThreadItemPayload::FileChange { id, status, .. } => (id, "apply_patch".to_owned(), serde_json::to_value(status)? == json!("completed")),
        // Dynamic calls are observed at the request/response boundary once.
        _ => return Ok(()),
    };
    if completed {
        handler.on_tool_finished(&InhouseToolResultEnvelope {
            call_id: id.clone(), tool_name: name, arguments: Value::Null, result_text: String::new(), ok,
            argument_correction_allowed: false, error_kind: None, retry_guidance: None,
        }).await?;
    } else {
        handler.on_tool_started(&InhouseToolCallEnvelope {
            call_id: id.clone(), provider_call_id: Some(id.clone()), tool_name: name, arguments: Value::Null,
        }).await?;
    }
    Ok(())
}

fn item_started_status(item: &ThreadItem) -> Option<(&'static str, String)> {
    match &item.payload {
        ThreadItemPayload::Reasoning { .. } => Some(("thinking", "Thinking…".to_string())),
        ThreadItemPayload::CommandExecution { command, .. } => {
            Some(("tool", format!("Running command: {command}")))
        }
        ThreadItemPayload::McpToolCall { server, tool, .. } => {
            Some(("tool", format!("Calling tool: {server}/{tool}")))
        }
        ThreadItemPayload::DynamicToolCall { tool, .. } => Some(("tool", format!("Calling tool: {tool}"))),
        ThreadItemPayload::FileChange { changes, .. } => Some((
            "tool",
            format!("Applying file changes: {} file(s)", changes.len()),
        )),
        _ => None,
    }
}

fn item_completed_status(item: &ThreadItem) -> Option<(&'static str, String)> {
    match &item.payload {
        ThreadItemPayload::Reasoning {
            summary, content, ..
        } => {
            let text = summary
                .iter()
                .find(|entry| !entry.trim().is_empty())
                .cloned()
                .or_else(|| {
                    content
                        .iter()
                        .find(|entry| !entry.trim().is_empty())
                        .cloned()
                })
                .unwrap_or_else(|| "Thinking complete.".to_string());
            Some(("thinking", text))
        }
        ThreadItemPayload::CommandExecution {
            command, exit_code, ..
        } => Some((
            "tool",
            match exit_code {
                Some(code) => format!("Command finished ({code}): {command}"),
                None => format!("Command finished: {command}"),
            },
        )),
        ThreadItemPayload::McpToolCall {
            server,
            tool,
            error,
            ..
        } => Some((
            "tool",
            if error.is_none() {
                format!("Tool finished: {server}/{tool}")
            } else {
                format!("Tool finished with issues: {server}/{tool}")
            },
        )),
        ThreadItemPayload::DynamicToolCall { tool, success, .. } => Some((
            "tool",
            if success.unwrap_or(false) {
                format!("Tool finished: {tool}")
            } else {
                format!("Tool finished with issues: {tool}")
            },
        )),
        ThreadItemPayload::FileChange { changes, .. } => Some((
            "tool",
            format!("File changes finished: {} file(s)", changes.len()),
        )),
        _ => None,
    }
}

fn plan_update_text(explanation: Option<&str>, plan: &[TurnPlanStep]) -> String {
    let mut parts = Vec::new();
    if let Some(explanation) = explanation.map(str::trim).filter(|text| !text.is_empty()) {
        parts.push(explanation.to_string());
    }
    if let Some(active) = plan
        .iter()
        .find(|step| step.status == TurnPlanStepStatus::InProgress)
    {
        let step = active.step.trim();
        if !step.is_empty() {
            parts.push(format!("In progress: {step}"));
        }
    }
    if parts.is_empty() && !plan.is_empty() {
        let summary = plan
            .iter()
            .take(3)
            .filter_map(|step| {
                let step_text = step.step.trim();
                if step_text.is_empty() {
                    return None;
                }
                let marker = match step.status {
                    TurnPlanStepStatus::Completed => "[done]",
                    TurnPlanStepStatus::InProgress => "[now]",
                    TurnPlanStepStatus::Pending => "[next]",
                };
                Some(format!("{marker} {step_text}"))
            })
            .collect::<Vec<_>>()
            .join(" | ");
        if !summary.is_empty() {
            parts.push(summary);
        }
    }
    if parts.is_empty() {
        "Plan updated.".to_string()
    } else {
        format!("Plan: {}", parts.join(" | "))
    }
}

pub(crate) fn effective_model_provider(
    model_provider: Option<&str>,
    oss_mode: Option<&str>,
) -> Option<String> {
    model_provider
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            if oss_mode == Some("lmstudio") {
                Some("lmstudio".to_string())
            } else {
                None
            }
        })
}

pub(crate) fn effective_provider_base_url(
    provider_base_url: Option<&str>,
    lmstudio_host: Option<&str>,
) -> Option<String> {
    provider_base_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| normalize_lmstudio_base_url(lmstudio_host))
}

fn build_thread_request_config_overrides(
    model_provider: Option<&str>,
    provider_base_url: Option<&str>,
    prompt_user_instructions: Option<&str>,
    memory_overrides: Option<ThreadMemoryConfigOverrides>,
) -> Option<HashMap<String, Value>> {
    let mut overrides = HashMap::new();

    if let (Some(provider_id), Some(base_url)) = (model_provider, provider_base_url) {
        let provider_id = provider_id.trim();
        let base_url = base_url.trim();
        if !provider_id.is_empty() && !base_url.is_empty() {
            let provider_value = json!({
                "name": provider_id,
                "base_url": base_url,
                "wire_api": "responses",
                "requires_openai_auth": false,
            });
            overrides.insert(
                "model_provider".to_string(),
                Value::String(provider_id.to_string()),
            );
            overrides.insert(format!("model_providers.{provider_id}"), provider_value);
            if provider_uses_function_only_responses_tools(provider_id) {
                apply_function_only_responses_tool_overrides(&mut overrides);
            }
        }
    }

    if let Some(user_instructions) = prompt_user_instructions {
        overrides.insert(
            "user_instructions".to_string(),
            Value::String(user_instructions.to_string()),
        );
    }

    if let Some(memory_overrides) = memory_overrides {
        overrides.insert("features.memories".to_string(), Value::Bool(true));
        if let Some(generate_memories) = memory_overrides.generate_memories {
            overrides.insert(
                "memories.generate_memories".to_string(),
                Value::Bool(generate_memories),
            );
        }
        if let Some(use_memories) = memory_overrides.use_memories {
            overrides.insert(
                "memories.use_memories".to_string(),
                Value::Bool(use_memories),
            );
        }
        if let Some(min_rollout_idle_hours) = memory_overrides.min_rollout_idle_hours {
            overrides.insert(
                "memories.min_rollout_idle_hours".to_string(),
                Value::from(min_rollout_idle_hours),
            );
        }
        if let Some(extract_model) = memory_overrides.extract_model.as_deref() {
            overrides.insert(
                "memories.extract_model".to_string(),
                Value::String(extract_model.to_string()),
            );
        }
        if let Some(consolidation_model) = memory_overrides.consolidation_model.as_deref() {
            overrides.insert(
                "memories.consolidation_model".to_string(),
                Value::String(consolidation_model.to_string()),
            );
        }
    }

    if overrides.is_empty() {
        None
    } else {
        Some(overrides)
    }
}

fn provider_uses_function_only_responses_tools(provider_id: &str) -> bool {
    matches!(
        provider_id.trim().to_ascii_lowercase().as_str(),
        "local" | "lmstudio" | "lms"
    )
}

fn apply_function_only_responses_tool_overrides(overrides: &mut HashMap<String, Value>) {
    for feature in [
        "apps",
        "goals",
        "image_generation",
        "multi_agent",
        "plugins",
        "shell_tool",
        "tool_suggest",
    ] {
        overrides.insert(format!("features.{feature}"), Value::Bool(false));
    }
    overrides.insert(
        "web_search".to_string(),
        Value::String("disabled".to_string()),
    );
}

fn merge_thread_request_config_overrides(
    base: Option<HashMap<String, Value>>,
    extra: Option<HashMap<String, Value>>,
) -> Option<HashMap<String, Value>> {
    let mut merged = base.unwrap_or_default();
    if let Some(extra) = extra {
        merged.extend(extra);
    }
    (!merged.is_empty()).then_some(merged)
}

fn finalize_thread_request_config_overrides(
    codex_mode: CodexMode,
    base: Option<HashMap<String, Value>>,
    extra: Option<HashMap<String, Value>>,
) -> Option<HashMap<String, Value>> {
    let mut merged = merge_thread_request_config_overrides(base, extra).unwrap_or_default();
    if codex_mode.is_conversation() {
        for feature in crate::codex_app_server_bootstrap::conversation_mode_default_feature_keys() {
            merged.insert(format!("features.{feature}"), Value::Bool(false));
        }
        for value in merged
            .iter_mut()
            .filter(|(key, _)| key.starts_with("features."))
            .map(|(_, value)| value)
        {
            *value = Value::Bool(false);
        }
        for key in crate::codex_app_server_bootstrap::CONVERSATION_DISABLED_CONFIG_KEYS {
            merged.insert((*key).to_string(), Value::Bool(false));
        }
        merged.insert(
            "web_search".to_string(),
            Value::String("disabled".to_string()),
        );
        merged.retain(|key, _| {
            key != "mcp_servers"
                && !key.starts_with("mcp_servers.")
                && key != "project_doc_max_bytes"
                && key != "project_doc_fallback_filenames"
        });
    }
    (!merged.is_empty()).then_some(merged)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ThreadMemoryConfigOverrides {
    generate_memories: Option<bool>,
    use_memories: Option<bool>,
    min_rollout_idle_hours: Option<i64>,
    extract_model: Option<String>,
    consolidation_model: Option<String>,
}



fn memory_overrides_from_config(cfg: &CodexAgentConfig) -> Option<ThreadMemoryConfigOverrides> {
    if !cfg.memory_tool_enabled
        && cfg.generate_memories.is_none()
        && cfg.use_memories.is_none()
        && cfg.min_rollout_idle_hours.is_none()
        && cfg.memory_extract_model.is_none()
        && cfg.memory_consolidation_model.is_none()
    {
        return None;
    }
    Some(ThreadMemoryConfigOverrides {
        generate_memories: cfg.generate_memories,
        use_memories: cfg.use_memories,
        min_rollout_idle_hours: cfg.min_rollout_idle_hours,
        extract_model: cfg.memory_extract_model.clone(),
        consolidation_model: cfg.memory_consolidation_model.clone(),
    })
}

fn normalize_lmstudio_base_url(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.ends_with("/v1") {
        Some(trimmed.to_string())
    } else {
        Some(format!("{trimmed}/v1"))
    }
}

fn parse_approval_policy(policy: Option<&str>) -> anyhow::Result<Option<AskForApproval>> {
    policy
        .map(|policy| {
            serde_json::from_value(Value::String(policy.to_string()))
                .context("parse codex approval policy")
        })
        .transpose()
}

fn parse_reasoning_effort(policy: Option<&str>) -> anyhow::Result<Option<ReasoningEffort>> {
    policy
        .map(|value| {
            serde_json::from_value(Value::String(value.to_string()))
                .context("parse codex reasoning effort")
        })
        .transpose()
}

fn parse_sandbox_policy(value: Option<Value>) -> anyhow::Result<Option<SandboxPolicy>> {
    value
        .map(|value| serde_json::from_value(value).context("parse codex sandbox policy"))
        .transpose()
}

fn sandbox_mode_from_policy(value: Option<&Value>) -> anyhow::Result<Option<SandboxMode>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let policy_type = value.get("type").and_then(Value::as_str);
    let mode = match policy_type {
        Some("dangerFullAccess") => Some(SandboxMode::DangerFullAccess),
        Some("readOnly") => Some(SandboxMode::ReadOnly),
        Some("workspaceWrite") => Some(SandboxMode::WorkspaceWrite),
        _ => None,
    };
    Ok(mode)
}

async fn build_user_inputs(
    body_json: &str,
    _cwd: Option<&PathBuf>,
) -> anyhow::Result<Vec<UserInput>> {
    let parsed = serde_json::from_str::<Value>(body_json).ok();
    let mut typed_items = Vec::new();
    let mut document_notes = Vec::new();
    let mut primary_text = None::<String>;
    let mut image_count = 0usize;

    if let Some(value) = parsed.as_ref() {
        if let Some(text) = value.get("text").and_then(Value::as_str) {
            if !text.trim().is_empty() {
                primary_text = Some(text.to_string());
            }
        }
        // Support both legacy "attachments" and new "inline_attachments"/"external_attachments"
        let all_attachments = value
            .get("attachments")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().collect::<Vec<_>>())
            .unwrap_or_default();
        let inline_attachments = value
            .get("inline_attachments")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().collect::<Vec<_>>())
            .unwrap_or_default();
        let external_attachments = value
            .get("external_attachments")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().collect::<Vec<_>>())
            .unwrap_or_default();

        for attachment in all_attachments
            .into_iter()
            .chain(inline_attachments)
            .chain(external_attachments)
        {
            let kind = attachment.get("kind").and_then(Value::as_str).unwrap_or("");
            if kind == "image" {
                image_count += 1;
                let img_name = attachment
                    .get("name")
                    .or_else(|| attachment.get("filename"))
                    .and_then(Value::as_str)
                    .unwrap_or("image");
                // Try to extract image content for LLM processing
                if let Some(data_url) = attachment.get("data_url").and_then(Value::as_str) {
                    if data_url.starts_with("data:image/") {
                        match validate_inline_image_data_url(data_url) {
                            Ok(_) => {
                                typed_items.push(upstream_compat::inline_image_input(
                                    data_url.to_string(),
                                ));
                                continue;
                            }
                            Err(err) => {
                                document_notes.push(format!(
                                    "Note: Image attachment '{}' was attached by user but was not sent to the model: {}.",
                                    img_name, err
                                ));
                                continue;
                            }
                        }
                    }
                }
                // Fallback: acknowledge image was received but couldn't extract
                document_notes.push(format!(
                    "Note: Image attachment '{}' was attached by user but couldn't be processed.",
                    img_name
                ));
                continue;
            }
            if kind == "text" || kind == "document" {
                let name = attachment
                    .get("name")
                    .or_else(|| attachment.get("filename"))
                    .and_then(Value::as_str)
                    .unwrap_or("unnamed-document");
                let mime = attachment
                    .get("mime_type")
                    .and_then(Value::as_str)
                    .unwrap_or("application/octet-stream");
                // For inline text attachments with data_url, try to extract content
                if let Some(data_url) = attachment.get("data_url").and_then(Value::as_str) {
                    if let Some(text_content) = decode_inline_text_data_url(data_url) {
                        document_notes.push(format!(
                            "Attached document: {} ({})\n\nContent:\n{}",
                            name, mime, text_content
                        ));
                        continue;
                    }
                }
                // Fall back to path/url reference if no inline content
                let location = attachment
                    .get("path")
                    .and_then(Value::as_str)
                    .or_else(|| attachment.get("url").and_then(Value::as_str))
                    .or_else(|| {
                        attachment.get("data_url").and_then(Value::as_str).map(|s| {
                            if s.len() > 50 {
                                &s[..50]
                            } else {
                                s
                            }
                        })
                    })
                    .or_else(|| attachment.get("telegram_file_id").and_then(Value::as_str))
                    .unwrap_or("unavailable");
                document_notes.push(format!(
                    "Attached document: {} ({}) at {}",
                    name, mime, location
                ));
            }
        }
    }

    if primary_text.is_some() || image_count > 0 || !document_notes.is_empty() {
        let text = match (
            primary_text.take(),
            image_count > 0,
            document_notes.is_empty(),
        ) {
            (Some(text), _, true) => text,
            (Some(text), _, false) => format!("{text}\n\n{}", document_notes.join("\n")),
            (None, true, true) => image_only_prompt_text(image_count),
            (None, true, false) => format!(
                "{}\n{}",
                image_only_prompt_text(image_count),
                document_notes.join("\n")
            ),
            (None, false, false) => document_notes.join("\n"),
            (None, false, true) => String::new(),
        };
        if !text.trim().is_empty() {
            typed_items.insert(
                0,
                UserInput::Text {
                    text,
                    text_elements: Vec::new(),
                },
            );
        }
    }

    if typed_items.is_empty() {
        typed_items.push(UserInput::Text {
            text: body_json.to_string(),
            text_elements: Vec::new(),
        });
    }

    Ok(typed_items)
}

fn decode_inline_text_data_url(data_url: &str) -> Option<String> {
    let (header, encoded) = data_url.split_once(',')?;
    let header = header.strip_prefix("data:")?;
    let mut parts = header.split(';');
    let mime_type = parts.next()?.trim().to_ascii_lowercase();
    if !matches!(
        mime_type.as_str(),
        "text/plain" | "text/markdown" | "text/csv" | "application/json"
    ) {
        return None;
    }
    if !parts.any(|part| part.eq_ignore_ascii_case("base64")) {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .ok()?;
    let text = String::from_utf8(decoded).ok()?;
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn image_only_prompt_text(image_count: usize) -> String {
    match image_count {
        0 => String::new(),
        1 => "The operator attached 1 image and no text. This is not an empty message. Inspect the image and respond to its visible contents. Do not say the message is empty."
            .to_string(),
        count => format!(
            "The operator attached {count} images and no text. This is not an empty message. Inspect the images and respond to their visible contents. Do not say the message is empty."
        ),
    }
}

fn validate_inline_image_data_url(data_url: &str) -> anyhow::Result<()> {
    let (mime, bytes) = decode_inline_image_data_url(data_url)?;
    let (width, height) = image_dimensions(mime, &bytes)
        .with_context(|| format!("unsupported or invalid image data for {mime}"))?;
    validate_image_bounds(mime, bytes.len(), width, height)
}

fn decode_inline_image_data_url(data_url: &str) -> anyhow::Result<(&str, Vec<u8>)> {
    let (header, encoded) = data_url
        .split_once(',')
        .context("invalid data URL image: missing comma")?;
    if !header.starts_with("data:image/") {
        return Err(anyhow::anyhow!(
            "invalid data URL image: unsupported mime header"
        ));
    }
    if !header.ends_with(";base64") {
        return Err(anyhow::anyhow!(
            "invalid data URL image: expected base64 payload"
        ));
    }
    let mime = header
        .strip_prefix("data:")
        .and_then(|value| value.strip_suffix(";base64"))
        .context("invalid data URL image: malformed mime")?;
    if !matches!(
        mime,
        "image/png" | "image/jpeg" | "image/webp" | "image/gif"
    ) {
        return Err(anyhow::anyhow!("unsupported image mime type {mime}"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .context("decode data URL image")?;
    Ok((mime, bytes))
}

fn validate_image_bounds(
    mime: &str,
    byte_len: usize,
    width: u32,
    height: u32,
) -> anyhow::Result<()> {
    if byte_len == 0 {
        return Err(anyhow::anyhow!("empty image"));
    }
    if byte_len > INLINE_IMAGE_MAX_BYTES {
        return Err(anyhow::anyhow!(
            "image is too large: {} bytes > {} bytes",
            byte_len,
            INLINE_IMAGE_MAX_BYTES
        ));
    }
    if width < INLINE_IMAGE_MIN_DIMENSION || height < INLINE_IMAGE_MIN_DIMENSION {
        return Err(anyhow::anyhow!(
            "image dimensions are too small for model input: {width}x{height}"
        ));
    }
    if width > INLINE_IMAGE_MAX_DIMENSION || height > INLINE_IMAGE_MAX_DIMENSION {
        return Err(anyhow::anyhow!(
            "image dimensions are too large for model input: {width}x{height}"
        ));
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > INLINE_IMAGE_MAX_PIXELS {
        return Err(anyhow::anyhow!(
            "image pixel count is too large for model input: {pixels}"
        ));
    }
    debug!(
        mime,
        byte_len, width, height, "validated inline image attachment for codex input"
    );
    Ok(())
}

fn image_dimensions(mime: &str, bytes: &[u8]) -> anyhow::Result<(u32, u32)> {
    match mime {
        "image/png" => png_dimensions(bytes),
        "image/jpeg" => jpeg_dimensions(bytes),
        "image/gif" => gif_dimensions(bytes),
        "image/webp" => webp_dimensions(bytes),
        _ => Err(anyhow::anyhow!("unsupported image mime type {mime}")),
    }
}

fn png_dimensions(bytes: &[u8]) -> anyhow::Result<(u32, u32)> {
    const PNG_SIG: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != PNG_SIG || &bytes[12..16] != b"IHDR" {
        return Err(anyhow::anyhow!("invalid PNG header"));
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().expect("slice length"));
    let height = u32::from_be_bytes(bytes[20..24].try_into().expect("slice length"));
    Ok((width, height))
}

fn gif_dimensions(bytes: &[u8]) -> anyhow::Result<(u32, u32)> {
    if bytes.len() < 10 || (&bytes[..6] != b"GIF87a" && &bytes[..6] != b"GIF89a") {
        return Err(anyhow::anyhow!("invalid GIF header"));
    }
    let width = u16::from_le_bytes(bytes[6..8].try_into().expect("slice length")) as u32;
    let height = u16::from_le_bytes(bytes[8..10].try_into().expect("slice length")) as u32;
    Ok((width, height))
}

fn jpeg_dimensions(bytes: &[u8]) -> anyhow::Result<(u32, u32)> {
    if bytes.len() < 4 || bytes[0] != 0xff || bytes[1] != 0xd8 {
        return Err(anyhow::anyhow!("invalid JPEG header"));
    }
    let mut index = 2usize;
    while index + 9 <= bytes.len() {
        while index < bytes.len() && bytes[index] == 0xff {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        let marker = bytes[index];
        index += 1;
        if marker == 0xd9 || marker == 0xda {
            break;
        }
        if index + 2 > bytes.len() {
            break;
        }
        let segment_len =
            u16::from_be_bytes(bytes[index..index + 2].try_into().expect("slice length")) as usize;
        if segment_len < 2 || index + segment_len > bytes.len() {
            return Err(anyhow::anyhow!("invalid JPEG segment length"));
        }
        if matches!(
            marker,
            0xc0 | 0xc1
                | 0xc2
                | 0xc3
                | 0xc5
                | 0xc6
                | 0xc7
                | 0xc9
                | 0xca
                | 0xcb
                | 0xcd
                | 0xce
                | 0xcf
        ) {
            if segment_len < 7 {
                return Err(anyhow::anyhow!("invalid JPEG SOF segment"));
            }
            let height = u16::from_be_bytes(
                bytes[index + 3..index + 5]
                    .try_into()
                    .expect("slice length"),
            ) as u32;
            let width = u16::from_be_bytes(
                bytes[index + 5..index + 7]
                    .try_into()
                    .expect("slice length"),
            ) as u32;
            return Ok((width, height));
        }
        index += segment_len;
    }
    Err(anyhow::anyhow!("JPEG dimensions not found"))
}

fn webp_dimensions(bytes: &[u8]) -> anyhow::Result<(u32, u32)> {
    if bytes.len() < 30 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return Err(anyhow::anyhow!("invalid WebP header"));
    }
    match &bytes[12..16] {
        b"VP8X" => {
            let width = 1
                + u32::from(bytes[24])
                + (u32::from(bytes[25]) << 8)
                + (u32::from(bytes[26]) << 16);
            let height = 1
                + u32::from(bytes[27])
                + (u32::from(bytes[28]) << 8)
                + (u32::from(bytes[29]) << 16);
            Ok((width, height))
        }
        b"VP8 " => {
            if bytes.len() < 30 || bytes[23] != 0x9d || bytes[24] != 0x01 || bytes[25] != 0x2a {
                return Err(anyhow::anyhow!("invalid WebP VP8 frame header"));
            }
            let width =
                u16::from_le_bytes(bytes[26..28].try_into().expect("slice length")) as u32 & 0x3fff;
            let height =
                u16::from_le_bytes(bytes[28..30].try_into().expect("slice length")) as u32 & 0x3fff;
            Ok((width, height))
        }
        b"VP8L" => {
            if bytes.len() < 25 || bytes[20] != 0x2f {
                return Err(anyhow::anyhow!("invalid WebP VP8L frame header"));
            }
            let bits = u32::from(bytes[21])
                | (u32::from(bytes[22]) << 8)
                | (u32::from(bytes[23]) << 16)
                | (u32::from(bytes[24]) << 24);
            let width = (bits & 0x3fff) + 1;
            let height = ((bits >> 14) & 0x3fff) + 1;
            Ok((width, height))
        }
        _ => Err(anyhow::anyhow!("unsupported WebP chunk")),
    }
}


