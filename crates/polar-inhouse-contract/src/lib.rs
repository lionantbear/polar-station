mod backend_auth;
mod reply_generation;
pub mod session_environment;
pub use backend_auth::{InhouseBackendAuthAction, InhouseBackendAuthInput, InhouseBackendAuthView};
pub use polar_lgp_model::InhousePromptLayer;
pub use reply_generation::{ReplyGenerationView, ReplyMessageKey, MessageActivityQuery, MessageActivityPage, ToolActivityObservation};
pub use session_environment::{
    BoundPromptTool, SessionEnvironmentRef, ToolActor, ToolCallContext, ToolSessionScope,
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::Notify;

#[async_trait::async_trait]
pub trait InhouseProviderAttemptGate: Send + Sync {
    /// Persists one concrete outbound request before the adapter may contact
    /// its provider. The returned id is the durable attempt reference.
    async fn prepare_provider_attempt(
        &self,
        backend_driver: &str,
        backend_operation: &str,
        provider_request: &Value,
    ) -> anyhow::Result<String>;
}

#[derive(Clone, Default)]
pub struct InhouseProviderAttemptGateHandle(pub Option<Arc<dyn InhouseProviderAttemptGate>>);

impl fmt::Debug for InhouseProviderAttemptGateHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("InhouseProviderAttemptGateHandle")
            .field(&self.0.is_some())
            .finish()
    }
}

impl PartialEq for InhouseProviderAttemptGateHandle {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (None, None) => true,
            (Some(left), Some(right)) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }
}

impl Eq for InhouseProviderAttemptGateHandle {}

impl InhouseProviderAttemptGateHandle {
    pub async fn prepare(
        &self,
        backend_driver: &str,
        backend_operation: &str,
        provider_request: &Value,
    ) -> anyhow::Result<Option<String>> {
        match self.0.as_ref() {
            Some(gate) => gate
                .prepare_provider_attempt(backend_driver, backend_operation, provider_request)
                .await
                .map(Some),
            None => Ok(None),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexRuntimeBackend {
    Stdio,
    #[default]
    InProcess,
}

/// Native Codex home coordination, not resumable conversation state. Matches
/// the pinned thread-store writer-lock contract; never apply to other backends.
pub const CODEX_TRANSIENT_HOME_DIRECTORIES: &[&str] = &["thread-writer-locks"];

/// Provider-neutral host connector reservation passed to backend adapters.
///
/// Runtime ownership layers populate canonical base URLs for connectors that
/// a backend must not select. The contract intentionally carries no Adam
/// endpoint, slot, model, or cache-control types.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseConnectorDenyPolicy {
    #[serde(default)]
    pub reserved_base_urls: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InhouseApprovalKind {
    CommandExecution {
        command: Option<String>,
        cwd: Option<PathBuf>,
    },
    FileChange {
        grant_root: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InhouseApprovalRequest {
    pub approval_key: String,
    pub inhouse_polar_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub reason: Option<String>,
    pub kind: InhouseApprovalKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InhouseApprovalDecision {
    Accept,
    AcceptForSession,
    Decline,
    Cancel,
}

#[async_trait::async_trait]
pub trait InhouseApprovalHandler: Send + Sync {
    async fn request_approval(
        &self,
        request: InhouseApprovalRequest,
    ) -> anyhow::Result<InhouseApprovalDecision>;
}

pub trait InhouseTurnEventHandler: Send {
    /// Await observation persistence before publishing tool activity.
    fn on_tool_started(&mut self, call: &InhouseToolCallEnvelope)
        -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>> {
        let result = self.on_tool_call(&call.tool_name, &call.arguments);
        Box::pin(async move { result })
    }

    fn on_tool_finished(&mut self, result: &InhouseToolResultEnvelope)
        -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>> {
        let result = self.on_tool_result(&result.tool_name, &result.arguments, &result.result_text);
        Box::pin(async move { result })
    }

    /// Emit one append-only answer text delta.
    ///
    /// Backend drivers may call this only when the upstream provider supplied a
    /// real delta or the driver has safely normalized an append-only snapshot
    /// into a delta. Drivers that only have final text must return that text as
    /// the turn result instead of emitting a streaming event.
    fn on_agent_text_delta(&mut self, delta: &str) -> anyhow::Result<()>;

    fn on_intermediate_status(&mut self, text: &str, phase: &str) -> anyhow::Result<()> {
        let _ = text;
        let _ = phase;
        Ok(())
    }

    /// An append-only status delta, distinct from the full status snapshot above.
    fn on_intermediate_status_delta(&mut self, delta: &str, phase: &str) -> anyhow::Result<()> {
        self.on_intermediate_status(delta, phase)
    }

    fn on_plan_update(&mut self, text: &str) -> anyhow::Result<()> {
        let _ = text;
        Ok(())
    }

    fn on_tool_call(&mut self, tool: &str, arguments: &Value) -> anyhow::Result<()> {
        let _ = tool;
        let _ = arguments;
        Ok(())
    }

    fn on_tool_result(
        &mut self,
        tool: &str,
        arguments: &Value,
        result_text: &str,
    ) -> anyhow::Result<()> {
        let _ = tool;
        let _ = arguments;
        let _ = result_text;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InhouseModelInputError {
    message: String,
    detail: Option<String>,
}

impl InhouseModelInputError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(message: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            detail: Some(detail.into()),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    pub fn user_message(&self) -> String {
        format!(
            "The configured model rejected this image input: {}",
            self.message
        )
    }
}

impl fmt::Display for InhouseModelInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "non-retryable model input error: {}", self.message)?;
        if let Some(detail) = &self.detail {
            write!(f, " ({})", detail)?;
        }
        Ok(())
    }
}

impl Error for InhouseModelInputError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InhouseBackendBusyError {
    kind: String,
    message: String,
    detail: Option<String>,
}

impl InhouseBackendBusyError {
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn kind(&self) -> &str {
        &self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    pub fn user_message(&self) -> String {
        format!("The in-house backend is busy: {}", self.message)
    }

    pub fn downcast_ref(error: &anyhow::Error) -> Option<&Self> {
        error.downcast_ref::<Self>()
    }
}

impl fmt::Display for InhouseBackendBusyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "recoverable backend busy: {}", self.message)?;
        if !self.kind.trim().is_empty() {
            write!(f, " [{}]", self.kind)?;
        }
        if let Some(detail) = &self.detail {
            write!(f, " ({detail})")?;
        }
        Ok(())
    }
}

impl Error for InhouseBackendBusyError {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InhouseDynamicToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(default)]
    pub defer_loading: bool,
}

#[async_trait::async_trait]
pub trait InhouseDynamicToolHandler: Send + Sync {
    fn specs(&self) -> Vec<InhouseDynamicToolSpec>;

    /// Stable, backend-neutral instructions bound with this execution surface.
    /// Consumed by the shared session prompt compiler, never as tool authority.
    fn session_prompt_blocks(&self) -> Vec<InhousePromptBlock> {
        Vec::new()
    }

    /// Prepared augmentation for this invocation, separate from initial instructions.
    fn turn_prompt_blocks(&self) -> Vec<InhousePromptBlock> {
        Vec::new()
    }

    fn thread_config_overrides(&self) -> Option<HashMap<String, Value>> {
        None
    }

    async fn handle_call(&self, tool: &str, arguments: Value) -> anyhow::Result<String>;
}

/// Provider-neutral classification for a Polar-owned tool execution failure.
/// Backends may marshal this failure, but the tool owner chooses its kind and
/// safe retry guidance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InhouseToolExecutionErrorKind {
    OptimisticConflict,
    InvalidInput,
    NotFound,
    ExecutionFailed,
}

impl InhouseToolExecutionErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OptimisticConflict => "optimistic_conflict",
            Self::InvalidInput => "invalid_input",
            Self::NotFound => "not_found",
            Self::ExecutionFailed => "execution_failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InhouseToolExecutionError {
    kind: InhouseToolExecutionErrorKind,
    message: String,
    retry_guidance: String,
    argument_correction_allowed: bool,
}

impl InhouseToolExecutionError {
    pub fn new(
        kind: InhouseToolExecutionErrorKind,
        message: impl Into<String>,
        retry_guidance: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            retry_guidance: retry_guidance.into(),
            argument_correction_allowed: false,
        }
    }

    /// Trusted tool adapters opt in only when correcting input cannot repeat effects.
    pub fn with_argument_correction(mut self) -> Self {
        self.argument_correction_allowed = matches!(self.kind, InhouseToolExecutionErrorKind::InvalidInput | InhouseToolExecutionErrorKind::NotFound);
        self
    }

    pub fn argument_correction_allowed(&self) -> bool { self.argument_correction_allowed }

    pub fn kind(&self) -> InhouseToolExecutionErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn retry_guidance(&self) -> &str {
        &self.retry_guidance
    }

    pub fn downcast_ref(error: &anyhow::Error) -> Option<&Self> {
        error.downcast_ref::<Self>()
    }
}

impl fmt::Display for InhouseToolExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Polar tool execution failed [{}]: {}",
            self.kind.as_str(),
            self.message
        )
    }
}

impl Error for InhouseToolExecutionError {}

pub struct InhouseTurnControl {
    stop_requested: AtomicBool,
    notify: Notify,
}

impl InhouseTurnControl {
    pub fn new() -> Self {
        Self {
            stop_requested: AtomicBool::new(false),
            notify: Notify::new(),
        }
    }

    pub fn reset(&self) {
        self.stop_requested.store(false, Ordering::SeqCst);
    }

    pub fn request_stop(&self) {
        self.stop_requested.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn stop_requested(&self) -> bool {
        self.stop_requested.load(Ordering::SeqCst)
    }

    pub async fn wait_for_stop(&self) {
        loop {
            let notified = self.notify.notified();
            if self.stop_requested() {
                return;
            }
            notified.await;
        }
    }
}

impl Default for InhouseTurnControl {
    fn default() -> Self {
        Self::new()
    }
}

/// Provider-neutral scheduling intent for an Inhouse turn.
///
/// This semantic hint crosses the shared runtime/backend boundary. It does not
/// expose Adam slot policy: Adam alone decides whether to admit, queue, or
/// snapshot an eligible residency.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InhouseTurnSchedulingPriority {
    #[default]
    ActiveThread,
    Background,
}

/// Preserve typed conversation items for backends that support that encoding.
/// Rendered remains the default for existing prompt projections.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InhouseConversationEncoding {
    #[default]
    Rendered,
    CanonicalMessages,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InhouseTurnOptions {
    pub conversation_encoding: InhouseConversationEncoding,
    /// Correlation for deferred completion, not an LGP Message identity.
    pub reply_generation_id: Option<polar_inhouse_model::ReplyGenerationId>,
    /// Shared backend request assembled above provider adapters.
    ///
    /// The legacy provider-prefixed fields below remain temporarily available
    /// while callers migrate. New runtime paths should populate this request so
    /// Adam, Codex, and Pi all consume the same semantic boundary.
    pub backend_turn_request: Option<InhouseBackendTurnRequest>,
    /// Durable gate called by an adapter immediately before every outbound
    /// provider attempt, including retries and tool continuations.
    pub provider_attempt_gate: InhouseProviderAttemptGateHandle,
    /// Shared scheduling semantics. Physical admission, queueing, and slot
    /// selection remain backend-runtime concerns.
    pub scheduling_priority: InhouseTurnSchedulingPriority,
    /// Canonical LGP compaction selected by the runtime before the backend
    /// request is compiled. Provider adapters consume only the resulting
    /// shared prompt segments; they do not interpret or rewrite this record.
    pub context_compaction: Option<polar_lgp_model::ContextCompactionRecord>,
    /// Selected Thread memory only; the full persistent memory stays queryable.
    pub thread_memory_context: Option<String>,
    /// Verified provider-neutral admission facts for the final wire prompt.
    pub context_budget: Option<InhouseBackendContextBudget>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub pi_thread_init_prompt: Option<String>,
    pub pi_turn_prompt: Option<String>,
    pub codex_thread_prompt_override_enabled: Option<bool>,
    pub codex_thread_base_instructions: Option<String>,
    pub codex_thread_developer_instructions: Option<String>,
    pub codex_thread_user_instructions: Option<String>,
    pub codex_turn_prompt: Option<String>,
    pub adam_thread_prompt_override_enabled: Option<bool>,
    pub adam_thread_base_instructions: Option<String>,
    pub adam_thread_developer_instructions: Option<String>,
    pub adam_thread_user_project_context: Option<String>,
    pub adam_turn_developer_instructions: Option<String>,
    pub adam_turn_input_prefix: Option<String>,
    pub adam_owner_id: Option<String>,
    pub adam_thread_id: Option<String>,
    pub adam_inhouse_polar_id: Option<String>,
    pub adam_backend_profile_id: Option<String>,
    pub adam_request_message_id: Option<String>,
    pub canonical_thread_messages: Vec<InhouseCanonicalThreadMessage>,
    pub web_bridge_owner_id: Option<String>,
    pub web_bridge_thread_id: Option<String>,
    pub web_bridge_inhouse_polar_id: Option<String>,
    pub web_bridge_backend_profile_id: Option<String>,
    pub web_bridge_request_message_id: Option<String>,
}

/// Deterministic token-budget policy owned above any concrete backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseCompactionPolicy {
    pub context_window_tokens: i64,
    pub reserve_tokens: i64,
    pub keep_recent_tokens: i64,
    /// Maximum readable-summary budget requested from the isolated compaction
    /// summarizer. This is independent of the retained-tail budget.
    #[serde(default = "default_compaction_summary_target_tokens")]
    pub summary_target_tokens: i64,
}

fn default_compaction_summary_target_tokens() -> i64 {
    1_024
}

/// Reason a canonical Polar compaction candidate was requested.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InhouseCompactionTrigger {
    Automatic {
        context_tokens: i64,
    },
    Manual {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        operator_instructions: Option<String>,
    },
}

/// Semantic boundary between the source messages summarized and the tail kept
/// verbatim in the next prompt projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseCompactionCutPoint {
    #[serde(default)]
    pub messages_to_summarize: Vec<String>,
    pub first_kept_message_id: String,
    #[serde(default)]
    pub split_turn: bool,
    #[serde(default)]
    pub split_turn_prefix_message_ids: Vec<String>,
}

/// Provider-neutral plan produced before summary generation or backend work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseCompactionPlan {
    pub candidate_compaction_id: String,
    pub thread_id: String,
    pub policy: InhouseCompactionPolicy,
    pub trigger: InhouseCompactionTrigger,
    pub cut_point: InhouseCompactionCutPoint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_compaction_ref: Option<String>,
    #[serde(default)]
    pub required_markers: Vec<String>,
    #[serde(default)]
    pub critical_context_refs: Vec<String>,
    pub target_context_compaction_epoch: i64,
}

/// Result of checking a readable summary against its canonical source refs and
/// required control markers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseCompactionQualityReport {
    pub accepted: bool,
    #[serde(default)]
    pub missing_required_markers: Vec<String>,
    #[serde(default)]
    pub fabricated_control_markers: Vec<String>,
    #[serde(default)]
    pub reasons: Vec<String>,
}

/// Readable semantic summary returned to Polar for canonical LGP persistence.
/// Backend-private opaque compaction artifacts deliberately do not belong here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseCompactionSummary {
    pub candidate_compaction_id: String,
    pub summary_text: String,
    #[serde(default)]
    pub source_refs: Vec<String>,
    pub tokens_before: i64,
    pub tokens_after_estimate: i64,
    pub quality_report: InhouseCompactionQualityReport,
}

/// Canonical authorization emitted only after the exact candidate record has
/// been committed to LGP. Backend cache/prepared-state adapters may use this
/// value for correlation, but it intentionally carries no slot or KV details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseCompactionCommit {
    pub thread_id: String,
    pub candidate_compaction_id: String,
    pub committed_compaction_id: String,
    pub previous_context_compaction_epoch: i64,
    pub context_compaction_epoch: i64,
    pub source_digest: String,
}

impl InhouseCompactionCommit {
    pub fn validate(&self) -> Result<(), String> {
        for (field, value) in [
            ("thread_id", self.thread_id.as_str()),
            (
                "candidate_compaction_id",
                self.candidate_compaction_id.as_str(),
            ),
            (
                "committed_compaction_id",
                self.committed_compaction_id.as_str(),
            ),
            ("source_digest", self.source_digest.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(format!("{field} must not be blank"));
            }
        }
        if self.candidate_compaction_id != self.committed_compaction_id {
            return Err(
                "committed_compaction_id must exactly match candidate_compaction_id".to_string(),
            );
        }
        let expected_epoch = self
            .previous_context_compaction_epoch
            .checked_add(1)
            .filter(|_| self.previous_context_compaction_epoch >= 0);
        if expected_epoch != Some(self.context_compaction_epoch) {
            return Err(
                "context_compaction_epoch must be exactly one greater than the previous epoch"
                    .to_string(),
            );
        }
        Ok(())
    }
}

/// Fail-closed cleanup signal for disposable backend candidate state. An abort
/// never changes canonical LGP truth and never authorizes cache promotion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseCompactionAbort {
    pub thread_id: String,
    pub candidate_compaction_id: String,
    pub target_context_compaction_epoch: i64,
    pub source_digest: String,
    pub reason: String,
}

impl InhouseCompactionAbort {
    pub fn validate(&self) -> Result<(), String> {
        for (field, value) in [
            ("thread_id", self.thread_id.as_str()),
            (
                "candidate_compaction_id",
                self.candidate_compaction_id.as_str(),
            ),
            ("source_digest", self.source_digest.as_str()),
            ("reason", self.reason.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(format!("{field} must not be blank"));
            }
        }
        if self.target_context_compaction_epoch <= 0 {
            return Err("target_context_compaction_epoch must be positive".to_string());
        }
        Ok(())
    }
}

/// Exact provider-neutral projection offered to an optional physical
/// compaction optimizer before LGP admits the candidate as canonical truth.
/// The request carries semantic correlation and the already-compiled backend
/// turn request, never a provider slot, lease, snapshot, or KV identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseCompactionPrepareRequest {
    pub thread_id: String,
    pub candidate_compaction_id: String,
    pub target_context_compaction_epoch: i64,
    pub source_digest: String,
    pub retained_tail_checkpoint_source_ref: String,
    pub backend_turn_request: InhouseBackendTurnRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InhouseCompactionPrepareDisposition {
    Prepared,
    NotPrepared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InhouseCompactionPreparedPromotion {
    Promoted,
    NotPrepared,
}

/// Provider-neutral port for disposable physical compaction acceleration.
///
/// Implementations own every provider-specific manager, store, projection,
/// slot, lease, snapshot, and KV type. The semantic runtime invokes this port
/// only with canonical prepare/commit/abort correlations.
#[async_trait::async_trait]
pub trait InhouseCompactionPhysicalPort: Send + Sync {
    async fn prepare_compact(
        &self,
        _request: &InhouseCompactionPrepareRequest,
    ) -> anyhow::Result<InhouseCompactionPrepareDisposition> {
        Ok(InhouseCompactionPrepareDisposition::NotPrepared)
    }

    async fn invalidate_compaction_cache_epoch(
        &self,
        commit: &InhouseCompactionCommit,
    ) -> anyhow::Result<()>;

    async fn promote_prepared_compact(
        &self,
        commit: &InhouseCompactionCommit,
    ) -> anyhow::Result<InhouseCompactionPreparedPromotion>;

    async fn discard_prepared_compact(&self, abort: &InhouseCompactionAbort) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseCanonicalThreadMessage {
    pub message_id: String,
    pub role: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseBackendTurnRef {
    pub owner_id: String,
    pub thread_id: String,
    pub inhouse_polar_id: String,
    pub request_message_id: String,
    pub backend_profile_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhousePromptBlock {
    pub block_id: String,
    pub layer: InhousePromptLayer,
    pub source_ref: String,
    pub body_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhousePromptSegment {
    pub segment_id: String,
    pub source_ref: String,
    pub role: String,
    pub rendered_prompt: String,
    pub segment_hash: String,
    pub compaction_epoch: i64,
}

pub type PromptSegment = InhousePromptSegment;

impl InhousePromptSegment {
    /// Creates a deterministic segment for a canonical non-message LGP source,
    /// such as a readable context compaction record.
    pub fn from_rendered_source(
        segment_id: impl Into<String>,
        source_ref: impl Into<String>,
        role: impl Into<String>,
        rendered_prompt: impl Into<String>,
        compaction_epoch: i64,
    ) -> Self {
        let rendered_prompt = rendered_prompt.into();
        Self {
            segment_id: segment_id.into(),
            source_ref: source_ref.into(),
            role: normalize_prompt_role(&role.into()),
            segment_hash: sha256_hex(&rendered_prompt),
            rendered_prompt,
            compaction_epoch,
        }
    }

    pub fn from_lgp_message(
        message_id: impl Into<String>,
        role: impl Into<String>,
        revision: impl Into<String>,
        text: impl AsRef<str>,
        compaction_epoch: i64,
    ) -> Self {
        let message_id = message_id.into();
        let role = normalize_prompt_role(&role.into());
        let revision = revision.into();
        let rendered_prompt =
            render_lgp_message_segment(&message_id, &role, &revision, text.as_ref());
        Self {
            segment_id: format!("lgp-message:{message_id}@{revision}"),
            source_ref: format!("lgp:message:{message_id}@{revision}"),
            role,
            segment_hash: sha256_hex(&rendered_prompt),
            rendered_prompt,
            compaction_epoch,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseMessageProjection {
    pub projection_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_turn_message_id: Option<String>,
    #[serde(default)]
    pub segments: Vec<InhousePromptSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolarPromptProjection {
    pub projection_id: String,
    pub rendered_prompt: String,
    pub full_prompt: String,
    pub compaction_epoch: i64,
    #[serde(default)]
    pub segments: Vec<PromptSegment>,
}

impl PolarPromptProjection {
    pub fn from_segments(
        prefix: impl AsRef<str>,
        segments: Vec<PromptSegment>,
        current_turn_message_id: Option<&str>,
    ) -> Self {
        let prefix = prefix.as_ref().trim();
        let rendered_segments = segments
            .iter()
            .map(|segment| segment.rendered_prompt.trim())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let rendered_prompt = [prefix, rendered_segments.as_str()]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let full_prompt = match current_turn_message_id {
            Some(message_id) if !message_id.trim().is_empty() => {
                format!(
                    "{}\n<current-turn message_id=\"{}\" />",
                    rendered_prompt,
                    escape_prompt_attr(message_id.trim())
                )
            }
            _ => rendered_prompt.clone(),
        };
        let compaction_epoch = segments
            .iter()
            .map(|segment| segment.compaction_epoch)
            .max()
            .unwrap_or_default();
        let projection_id = format!("polar-prompt-projection:{}", sha256_hex(&full_prompt));
        Self {
            projection_id,
            rendered_prompt,
            full_prompt,
            compaction_epoch,
            segments,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhousePromptEnvelope {
    pub envelope_id: String,
    #[serde(default)]
    pub blocks: Vec<InhousePromptBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_projection: Option<InhouseMessageProjection>,
}

/// Redacted hashes of one backend adapter's thread and turn renderings.
///
/// These hashes prove that adapters consumed the shared prompt envelope
/// without exposing the rendered prompt bytes themselves.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhousePromptRendererHashes {
    pub thread_render_hash: String,
    pub turn_render_hash: String,
}

/// Operator-readable parity evidence derived from one shared prompt envelope.
///
/// Prompt text, provider fields, attachment bodies, and credentials are
/// deliberately absent. `canonical_source_refs` are stable LGP/prompt source
/// addresses; the three renderer entries contain hashes only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhousePromptParityRendererHashes {
    pub adam: InhousePromptRendererHashes,
    pub codex: InhousePromptRendererHashes,
    pub pi: InhousePromptRendererHashes,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhousePromptParityTrace {
    pub envelope_id: String,
    #[serde(default)]
    pub canonical_source_refs: Vec<String>,
    #[serde(default)]
    pub canonical_source_ref_count: usize,
    pub renderer_hashes: InhousePromptParityRendererHashes,
}

/// Redacted proof emitted at the shared runtime -> backend-adapter dispatch
/// boundary for one concrete turn.
///
/// The receipt names the selected adapter and exact shared envelope while
/// retaining only canonical source addresses and hashes. A pre-provider
/// rejection sets `dispatch_status` to `rejected_pre_provider` and records
/// `provider_contact_attempted = false`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhousePromptAdapterReceipt {
    pub schema: String,
    pub backend_driver: String,
    pub dispatch_status: String,
    pub envelope_id: String,
    #[serde(default)]
    pub canonical_source_refs: Vec<String>,
    #[serde(default)]
    pub canonical_source_ref_count: usize,
    pub renderer_hashes: InhousePromptRendererHashes,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_contact_attempted: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseBackendToolSpec {
    pub name: String,
    pub description: String,
    #[serde(alias = "inputSchema")]
    pub input_schema: Value,
}

impl From<InhouseDynamicToolSpec> for InhouseBackendToolSpec {
    fn from(value: InhouseDynamicToolSpec) -> Self {
        Self {
            name: value.name,
            description: value.description,
            input_schema: value.input_schema,
        }
    }
}

impl From<&InhouseBackendToolSpec> for InhouseDynamicToolSpec {
    fn from(value: &InhouseBackendToolSpec) -> Self {
        Self {
            name: value.name.clone(),
            description: value.description.clone(),
            input_schema: value.input_schema.clone(),
            defer_loading: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseToolPlan {
    pub tool_plan_id: String,
    #[serde(default)]
    pub tools: Vec<InhouseBackendToolSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InhouseToolCallEnvelope {
    pub call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_call_id: Option<String>,
    pub tool_name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InhouseToolResultEnvelope {
    pub call_id: String,
    pub tool_name: String,
    pub arguments: Value,
    pub result_text: String,
    pub ok: bool,
    #[serde(default)]
    pub argument_correction_allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_guidance: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseBackendCapabilities {
    pub supports_prompt_envelope: bool,
    pub supports_tool_bridge: bool,
    pub supports_llama_slot_control: bool,
    pub supports_kv_residency: bool,
    pub supports_kv_snapshot_save: bool,
    pub supports_kv_snapshot_load: bool,
    pub supports_cache_affinity: bool,
    pub supports_runtime_evidence: bool,
}

impl InhouseBackendCapabilities {
    /// Historical native-Adam declaration. Not a runtime probe or the
    /// requirements of an ordinary turn; kept for persisted-contract fixtures.
    pub const fn adam() -> Self {
        Self {
            supports_prompt_envelope: true,
            supports_tool_bridge: true,
            supports_llama_slot_control: true,
            supports_kv_residency: true,
            // Snapshot actions are optional physical llama capabilities. The
            // Adam runtime fills them from a live endpoint probe; a generic
            // Adam request must not claim or require them statically.
            supports_kv_snapshot_save: false,
            supports_kv_snapshot_load: false,
            supports_cache_affinity: true,
            supports_runtime_evidence: true,
        }
    }

    pub const fn codex() -> Self {
        Self::conversation()
    }

    /// Provider-independent adapter support for ordinary conversation turns.
    /// Account entitlement and physical engine capabilities are not implied.
    pub const fn conversation() -> Self {
        Self {
            supports_prompt_envelope: true,
            supports_tool_bridge: true,
            supports_llama_slot_control: false,
            supports_kv_residency: false,
            supports_kv_snapshot_save: false,
            supports_kv_snapshot_load: false,
            supports_cache_affinity: false,
            supports_runtime_evidence: false,
        }
    }

    pub const fn pi() -> Self {
        Self::codex()
    }

    pub const fn intersection(&self, available: &Self) -> Self {
        Self {
            supports_prompt_envelope: self.supports_prompt_envelope
                && available.supports_prompt_envelope,
            supports_tool_bridge: self.supports_tool_bridge && available.supports_tool_bridge,
            supports_llama_slot_control: self.supports_llama_slot_control
                && available.supports_llama_slot_control,
            supports_kv_residency: self.supports_kv_residency && available.supports_kv_residency,
            supports_kv_snapshot_save: self.supports_kv_snapshot_save
                && available.supports_kv_snapshot_save,
            supports_kv_snapshot_load: self.supports_kv_snapshot_load
                && available.supports_kv_snapshot_load,
            supports_cache_affinity: self.supports_cache_affinity
                && available.supports_cache_affinity,
            supports_runtime_evidence: self.supports_runtime_evidence
                && available.supports_runtime_evidence,
        }
    }

    pub fn missing_from(&self, effective: &Self) -> Vec<&'static str> {
        [
            (
                "supports_prompt_envelope",
                self.supports_prompt_envelope,
                effective.supports_prompt_envelope,
            ),
            (
                "supports_tool_bridge",
                self.supports_tool_bridge,
                effective.supports_tool_bridge,
            ),
            (
                "supports_llama_slot_control",
                self.supports_llama_slot_control,
                effective.supports_llama_slot_control,
            ),
            (
                "supports_kv_residency",
                self.supports_kv_residency,
                effective.supports_kv_residency,
            ),
            (
                "supports_kv_snapshot_save",
                self.supports_kv_snapshot_save,
                effective.supports_kv_snapshot_save,
            ),
            (
                "supports_kv_snapshot_load",
                self.supports_kv_snapshot_load,
                effective.supports_kv_snapshot_load,
            ),
            (
                "supports_cache_affinity",
                self.supports_cache_affinity,
                effective.supports_cache_affinity,
            ),
            (
                "supports_runtime_evidence",
                self.supports_runtime_evidence,
                effective.supports_runtime_evidence,
            ),
        ]
        .into_iter()
        .filter_map(|(name, required, present)| (required && !present).then_some(name))
        .collect()
    }
}

/// Semantic requirements of an ordinary turn, distinct from adapter support
/// and from optional, probed physical-engine operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InhouseTurnRequirements {
    pub tool_bridge: bool,
}

impl InhouseTurnRequirements {
    pub fn from_tool_plan(plan: &InhouseToolPlan) -> Self {
        Self {
            tool_bridge: !plan.tools.is_empty(),
        }
    }
}

impl From<InhouseTurnRequirements> for InhouseBackendCapabilities {
    fn from(requirements: InhouseTurnRequirements) -> Self {
        Self {
            supports_prompt_envelope: true,
            supports_tool_bridge: requirements.tool_bridge,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseBackendCapabilityNegotiation {
    pub required_capabilities: InhouseBackendCapabilities,
    pub effective_capabilities: InhouseBackendCapabilities,
    #[serde(default)]
    pub missing_required_capabilities: Vec<String>,
}

impl InhouseBackendCapabilityNegotiation {
    pub fn negotiate(
        required_capabilities: InhouseBackendCapabilities,
        available_capabilities: InhouseBackendCapabilities,
    ) -> Self {
        let effective_capabilities = required_capabilities.intersection(&available_capabilities);
        let missing_required_capabilities = required_capabilities
            .missing_from(&effective_capabilities)
            .into_iter()
            .map(str::to_string)
            .collect();
        Self {
            required_capabilities,
            effective_capabilities,
            missing_required_capabilities,
        }
    }

    pub fn satisfied(&self) -> bool {
        self.missing_required_capabilities.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseBackendExtensionEnvelope {
    pub namespace: String,
    pub payload: Value,
}

/// Provider-neutral admission facts for one fully rendered backend prompt.
///
/// Physical provider identity (slots, leases, runtime ids and snapshots) must
/// remain behind the backend adapter and is intentionally absent here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseBackendContextBudget {
    pub max_context_tokens: i64,
    pub configured_context_tokens: i64,
    pub output_reserve_tokens: i64,
    pub safety_margin_tokens: i64,
    pub exact_prompt_tokens: i64,
    pub effective_input_budget_tokens: i64,
}

impl InhouseBackendContextBudget {
    pub fn from_verified_live_count(
        max_context_tokens: i64,
        configured_context_tokens: i64,
        output_reserve_tokens: i64,
        safety_margin_tokens: i64,
        exact_prompt_tokens: i64,
    ) -> Result<Self, &'static str> {
        if max_context_tokens <= 0 || configured_context_tokens <= 0 {
            return Err("verified context capacity is unavailable");
        }
        if output_reserve_tokens < 0 || safety_margin_tokens < 0 || exact_prompt_tokens < 0 {
            return Err("context budget token counts must be non-negative");
        }
        let effective_input_budget_tokens = max_context_tokens
            .min(configured_context_tokens)
            .checked_sub(output_reserve_tokens)
            .and_then(|value| value.checked_sub(safety_margin_tokens))
            .filter(|value| *value > 0)
            .ok_or("output reserve and safety margin exhaust live context capacity")?;
        Ok(Self {
            max_context_tokens,
            configured_context_tokens,
            output_reserve_tokens,
            safety_margin_tokens,
            exact_prompt_tokens,
            effective_input_budget_tokens,
        })
    }

    pub fn requires_compaction(self) -> bool {
        self.exact_prompt_tokens > self.effective_input_budget_tokens
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseBackendTurnRequest {
    pub turn_ref: InhouseBackendTurnRef,
    #[serde(default)]
    pub scheduling_priority: InhouseTurnSchedulingPriority,
    pub prompt_envelope: InhousePromptEnvelope,
    pub message_projection: InhouseMessageProjection,
    pub tool_plan: InhouseToolPlan,
    pub requested_capabilities: InhouseBackendCapabilities,
    #[serde(default)]
    pub effective_capabilities: InhouseBackendCapabilities,
    #[serde(default)]
    pub extensions: Vec<InhouseBackendExtensionEnvelope>,
}

/// Immutable session-scoped semantic context consumed by every backend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InhouseSessionContextV1 {
    pub schema: String,
    pub actor_ref: InhouseBackendTurnRef,
    pub backend_driver: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub prompt_blocks: Vec<InhousePromptBlock>,
    #[serde(default)]
    pub session_config: Value,
    pub capabilities: InhouseBackendCapabilities,
    #[serde(default)]
    pub source_provenance: Vec<String>,
    pub revision: String,
    pub session_context_hash: String,
}

/// Immutable invocation-scoped semantic context consumed by every backend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InhouseTurnContextV1 {
    pub schema: String,
    pub invocation_id: String,
    pub source_message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_turn_id: Option<String>,
    pub consumed_session_context_revision: String,
    pub consumed_session_context_hash: String,
    #[serde(default)]
    pub prompt_blocks: Vec<InhousePromptBlock>,
    pub message_projection: InhouseMessageProjection,
    pub tool_plan: InhouseToolPlan,
    #[serde(default)]
    pub supplemental_context: Value,
    #[serde(default)]
    pub source_provenance: Vec<String>,
    pub turn_context_hash: String,
}

/// Durable permit record written before one concrete provider request.
/// It deliberately makes no claim that contact has happened yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAttemptPreparedV1 {
    pub schema: String,
    pub attempt_id: String,
    pub invocation_id: String,
    pub ordinal: u32,
    pub backend_driver: String,
    pub backend_operation: String,
    pub request_hash: String,
    pub prepared_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAttemptOutcomeV1 {
    Responded,
    TransportError,
    CancelledAfterContact,
}

/// Immutable observation written after a provider request settles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAttemptFinishedV1 {
    pub schema: String,
    pub attempt_id: String,
    pub invocation_id: String,
    pub provider_contacted: bool,
    pub outcome: ProviderAttemptOutcomeV1,
    pub started_at_unix_ms: i64,
    pub finished_at_unix_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_response_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDispatchReceiptV1 {
    pub schema: String,
    pub backend_session_id: String,
    pub session_context_revision: String,
    pub session_context_hash: String,
    pub backend_operation: String,
    #[serde(default)]
    pub provider_attempt_refs: Vec<String>,
    pub provider_contacted: bool,
    pub dispatched_at_unix_ms: i64,
    pub completed_at_unix_ms: i64,
}

impl SessionDispatchReceiptV1 {
    pub fn validate(&self) -> Result<(), String> {
        validate_contact_receipt(
            self.provider_contacted,
            &self.provider_attempt_refs,
            self.dispatched_at_unix_ms,
            self.completed_at_unix_ms,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnDispatchReceiptV1 {
    pub schema: String,
    pub invocation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_turn_id: Option<String>,
    pub consumed_session_context_revision: String,
    pub consumed_session_context_hash: String,
    pub turn_context_hash: String,
    pub backend_operation: String,
    #[serde(default)]
    pub provider_attempt_refs: Vec<String>,
    pub provider_contacted: bool,
    pub dispatched_at_unix_ms: i64,
    pub completed_at_unix_ms: i64,
}

impl TurnDispatchReceiptV1 {
    pub fn validate(&self) -> Result<(), String> {
        validate_contact_receipt(
            self.provider_contacted,
            &self.provider_attempt_refs,
            self.dispatched_at_unix_ms,
            self.completed_at_unix_ms,
        )
    }
}

fn validate_contact_receipt(
    provider_contacted: bool,
    attempt_refs: &[String],
    dispatched_at: i64,
    completed_at: i64,
) -> Result<(), String> {
    if provider_contacted && attempt_refs.is_empty() {
        return Err("provider contact requires at least one finished attempt reference".into());
    }
    if completed_at < dispatched_at {
        return Err("receipt completion precedes dispatch".into());
    }
    Ok(())
}



/// Error returned when the compatibility adapter cannot recover a complete
/// shared turn identity from legacy provider-prefixed options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InhouseBackendTurnRequestBuildError {
    missing_fields: Vec<&'static str>,
}

impl InhouseBackendTurnRequestBuildError {
    pub fn missing_fields(&self) -> &[&'static str] {
        &self.missing_fields
    }
}

impl fmt::Display for InhouseBackendTurnRequestBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cannot build shared inhouse backend turn request; missing legacy fields: {}",
            self.missing_fields.join(", ")
        )
    }
}

impl Error for InhouseBackendTurnRequestBuildError {}

/// Single compatibility boundary for provider-prefixed legacy turn options.
/// New callers must construct `InhouseBackendTurnRequest` directly.
pub struct LegacyInhouseTurnOptionsAdapter<'a> {
    options: &'a InhouseTurnOptions,
}

/// Provider-neutral view of the remaining provider-prefixed prompt fields.
/// Only the legacy adapter may read those fields; production backends and
/// service adapters consume this view when a shared request is absent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyInhousePromptProjection {
    pub prompt_override_enabled: bool,
    pub system_instructions: Option<String>,
    pub developer_instructions: Option<String>,
    pub tool_usage_protocol: Option<String>,
    pub user_project_context: Option<String>,
    pub turn_augment: Option<String>,
}

impl<'a> LegacyInhouseTurnOptionsAdapter<'a> {
    pub fn new(options: &'a InhouseTurnOptions) -> Self {
        Self { options }
    }

    pub fn turn_ref_with_fallback(&self, fallback_inhouse_polar_id: &str) -> InhouseBackendTurnRef {
        InhouseBackendTurnRef {
            owner_id: self
                .options
                .adam_owner_id
                .clone()
                .or_else(|| self.options.web_bridge_owner_id.clone())
                .unwrap_or_else(|| "unknown-owner".to_string()),
            thread_id: self
                .options
                .adam_thread_id
                .clone()
                .or_else(|| self.options.web_bridge_thread_id.clone())
                .unwrap_or_else(|| "unknown-thread".to_string()),
            inhouse_polar_id: self
                .options
                .adam_inhouse_polar_id
                .clone()
                .or_else(|| self.options.web_bridge_inhouse_polar_id.clone())
                .unwrap_or_else(|| fallback_inhouse_polar_id.to_string()),
            request_message_id: self
                .options
                .adam_request_message_id
                .clone()
                .or_else(|| self.options.web_bridge_request_message_id.clone())
                .unwrap_or_else(|| "unknown-request-message".to_string()),
            backend_profile_id: self
                .options
                .adam_backend_profile_id
                .clone()
                .or_else(|| self.options.web_bridge_backend_profile_id.clone())
                .unwrap_or_else(|| "unknown-backend-profile".to_string()),
        }
    }

    pub fn current_turn_message_id(&self) -> Option<&str> {
        self.options
            .adam_request_message_id
            .as_deref()
            .or(self.options.web_bridge_request_message_id.as_deref())
    }

    pub fn backend_profile_id(&self) -> Option<&str> {
        self.options
            .adam_backend_profile_id
            .as_deref()
            .or(self.options.web_bridge_backend_profile_id.as_deref())
    }

    pub fn scheduling_priority(&self) -> InhouseTurnSchedulingPriority {
        self.options
            .backend_turn_request
            .as_ref()
            .map(|request| request.scheduling_priority)
            .unwrap_or(self.options.scheduling_priority)
    }

    pub fn prompt_projection(&self, backend_driver: &str) -> LegacyInhousePromptProjection {
        match backend_driver {
            "adam" => LegacyInhousePromptProjection {
                prompt_override_enabled: self
                    .options
                    .adam_thread_prompt_override_enabled
                    .unwrap_or(false),
                system_instructions: self.options.adam_thread_base_instructions.clone(),
                developer_instructions: self.options.adam_thread_developer_instructions.clone(),
                tool_usage_protocol: self.options.adam_turn_developer_instructions.clone(),
                user_project_context: self.options.adam_thread_user_project_context.clone(),
                turn_augment: self.options.adam_turn_input_prefix.clone(),
            },
            "codex" => LegacyInhousePromptProjection {
                prompt_override_enabled: self
                    .options
                    .codex_thread_prompt_override_enabled
                    .unwrap_or(false),
                system_instructions: self.options.codex_thread_base_instructions.clone(),
                developer_instructions: self.options.codex_thread_developer_instructions.clone(),
                tool_usage_protocol: None,
                user_project_context: self.options.codex_thread_user_instructions.clone(),
                turn_augment: self.options.codex_turn_prompt.clone(),
            },
            "pi" => LegacyInhousePromptProjection {
                prompt_override_enabled: true,
                system_instructions: self.options.pi_thread_init_prompt.clone(),
                developer_instructions: None,
                tool_usage_protocol: None,
                user_project_context: None,
                turn_augment: self.options.pi_turn_prompt.clone(),
            },
            _ => LegacyInhousePromptProjection::default(),
        }
    }

    pub fn prompt_blocks(&self, backend_driver: &str) -> Vec<InhousePromptBlock> {
        let projection = self.prompt_projection(backend_driver);
        [
            (
                InhousePromptLayer::System,
                "system",
                projection.system_instructions,
            ),
            (
                InhousePromptLayer::Developer,
                "developer",
                projection.developer_instructions,
            ),
            (
                InhousePromptLayer::ToolUsageProtocol,
                "tool-usage-protocol",
                projection.tool_usage_protocol,
            ),
            (
                InhousePromptLayer::UserProjectContext,
                "user-project-context",
                projection.user_project_context,
            ),
            (
                InhousePromptLayer::TurnAugment,
                "turn-augment",
                projection.turn_augment,
            ),
        ]
        .into_iter()
        .filter_map(|(layer, block_id, body)| {
            body.map(|body| body.trim().to_string())
                .filter(|body| !body.is_empty())
                .map(|body_text| InhousePromptBlock {
                    block_id: block_id.to_string(),
                    layer,
                    source_ref: format!("runtime:prompt:{backend_driver}:{block_id}"),
                    body_text,
                })
        })
        .collect()
    }

    pub fn turn_ref(&self) -> Result<InhouseBackendTurnRef, InhouseBackendTurnRequestBuildError> {
        let owner_id = legacy_turn_identity_value(
            self.options.adam_owner_id.as_deref(),
            self.options.web_bridge_owner_id.as_deref(),
        );
        let thread_id = legacy_turn_identity_value(
            self.options.adam_thread_id.as_deref(),
            self.options.web_bridge_thread_id.as_deref(),
        );
        let inhouse_polar_id = legacy_turn_identity_value(
            self.options.adam_inhouse_polar_id.as_deref(),
            self.options.web_bridge_inhouse_polar_id.as_deref(),
        );
        let request_message_id = legacy_turn_identity_value(
            self.options.adam_request_message_id.as_deref(),
            self.options.web_bridge_request_message_id.as_deref(),
        );
        let backend_profile_id = legacy_turn_identity_value(
            self.options.adam_backend_profile_id.as_deref(),
            self.options.web_bridge_backend_profile_id.as_deref(),
        );
        let missing_fields = [
            ("owner_id", owner_id.as_ref()),
            ("thread_id", thread_id.as_ref()),
            ("inhouse_polar_id", inhouse_polar_id.as_ref()),
            ("request_message_id", request_message_id.as_ref()),
            ("backend_profile_id", backend_profile_id.as_ref()),
        ]
        .into_iter()
        .filter_map(|(field, value)| value.is_none().then_some(field))
        .collect::<Vec<_>>();

        match (
            owner_id,
            thread_id,
            inhouse_polar_id,
            request_message_id,
            backend_profile_id,
        ) {
            (
                Some(owner_id),
                Some(thread_id),
                Some(inhouse_polar_id),
                Some(request_message_id),
                Some(backend_profile_id),
            ) => Ok(InhouseBackendTurnRef {
                owner_id,
                thread_id,
                inhouse_polar_id,
                request_message_id,
                backend_profile_id,
            }),
            _ => Err(InhouseBackendTurnRequestBuildError { missing_fields }),
        }
    }

    pub fn adapt(
        self,
        prompt_envelope: InhousePromptEnvelope,
        message_projection: InhouseMessageProjection,
        tool_plan: InhouseToolPlan,
        requested_capabilities: InhouseBackendCapabilities,
    ) -> Result<InhouseBackendTurnRequest, InhouseBackendTurnRequestBuildError> {
        Ok(InhouseBackendTurnRequest {
            turn_ref: self.turn_ref()?,
            scheduling_priority: self.options.scheduling_priority,
            prompt_envelope,
            message_projection,
            tool_plan,
            requested_capabilities,
            // Identity adaptation has no evidence about the bound runtime.
            effective_capabilities: InhouseBackendCapabilities::default(),
            extensions: Vec::new(),
        })
    }
}

impl InhouseBackendTurnRequest {
    /// Return an already assembled shared request or adapt the legacy identity
    /// fields while using the caller's canonical prompt/tool projections.
    pub fn from_turn_options(
        options: &InhouseTurnOptions,
        prompt_envelope: InhousePromptEnvelope,
        message_projection: InhouseMessageProjection,
        tool_plan: InhouseToolPlan,
        requested_capabilities: InhouseBackendCapabilities,
    ) -> Result<Self, InhouseBackendTurnRequestBuildError> {
        if let Some(request) = &options.backend_turn_request {
            return Ok(request.clone());
        }
        LegacyInhouseTurnOptionsAdapter::new(options).adapt(
            prompt_envelope,
            message_projection,
            tool_plan,
            requested_capabilities,
        )
    }
}

/// Compatibility entry point used while callers migrate from
/// `InhouseTurnOptions` to `InhouseBackendTurnRequest`.
pub fn build_backend_turn_request(
    options: &InhouseTurnOptions,
    prompt_envelope: InhousePromptEnvelope,
    message_projection: InhouseMessageProjection,
    tool_plan: InhouseToolPlan,
    requested_capabilities: InhouseBackendCapabilities,
) -> Result<InhouseBackendTurnRequest, InhouseBackendTurnRequestBuildError> {
    InhouseBackendTurnRequest::from_turn_options(
        options,
        prompt_envelope,
        message_projection,
        tool_plan,
        requested_capabilities,
    )
}

fn legacy_turn_identity_value(primary: Option<&str>, fallback: Option<&str>) -> Option<String> {
    primary
        .or(fallback)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn render_lgp_message_segment(message_id: &str, role: &str, revision: &str, text: &str) -> String {
    format!(
        "<lgp-message id=\"{}\" role=\"{}\" rev=\"{}\">\n{}\n</lgp-message>",
        escape_prompt_attr(message_id),
        escape_prompt_attr(role),
        escape_prompt_attr(revision),
        text.trim()
    )
}

fn normalize_prompt_role(role: &str) -> String {
    match role {
        "assistant" => "assistant".to_string(),
        "system" => "system".to_string(),
        "tool" => "tool".to_string(),
        _ => "user".to_string(),
    }
}

fn escape_prompt_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn sha256_hex(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InhouseProviderCacheEvidence {
    pub backend: String,
    pub wire_family: String,
    pub dialect: String,
    pub evidence_source: String,
    pub input_uncached_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_hit_rate: Option<f64>,
    pub cache_hit: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_cache_satisfied: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_slot_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

pub fn prepend_turn_prompt_to_input(input: &str, turn_prompt: Option<&str>) -> String {
    let Some(turn_prompt) = turn_prompt.map(str::trim).filter(|value| !value.is_empty()) else {
        return input.to_string();
    };
    let Ok(value) = serde_json::from_str::<Value>(input) else {
        return json!({
            "text": format!("{turn_prompt}\n\nUser message:\n{input}"),
        })
        .to_string();
    };

    match value {
        Value::Object(mut map) => {
            let existing_text = map
                .get("text")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("");
            let combined_text = if existing_text.is_empty() {
                turn_prompt.to_string()
            } else {
                format!("{turn_prompt}\n\nUser message:\n{existing_text}")
            };
            map.insert("text".to_string(), Value::String(combined_text));
            Value::Object(map).to_string()
        }
        _ => json!({
            "text": format!("{turn_prompt}\n\nUser message:\n{input}"),
        })
        .to_string(),
    }
}

#[derive(Debug, Clone, PartialEq)]
// Keep the provider-neutral outcome shape source-compatible across backend
// adapters; boxing only one variant would impose allocation and API churn on
// every runner for a size difference driven by optional evidence fields.
#[allow(clippy::large_enum_variant)]
pub enum InhouseAgentTurnOutcome {
    Immediate {
        session_id: String,
        reply_text: String,
    },
    ImmediateOutput(InhouseAgentTurnOutput),
    Deferred {
        session_id: String,
        status: String,
        status_text: String,
        metadata: Value,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InhouseTurnArtifacts {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thinking_blocks: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cache_evidence: Vec<InhouseProviderCacheEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_parity_trace: Option<InhousePromptParityTrace>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_adapter_receipt: Option<InhousePromptAdapterReceipt>,
}

impl InhouseTurnArtifacts {
    pub fn is_empty(&self) -> bool {
        self.thinking_blocks.is_empty()
            && self.cache_evidence.is_empty()
            && self.prompt_parity_trace.is_none()
            && self.prompt_adapter_receipt.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseIncompleteTurn {
    pub error_kind: String,
    pub status_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_error_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_error_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_guidance: Option<String>,
}

impl InhouseIncompleteTurn {
    pub fn new(error_kind: impl Into<String>, status_text: impl Into<String>) -> Self {
        Self {
            error_kind: error_kind.into(),
            status_text: status_text.into(),
            tool_error_kind: None,
            provider_error_kind: None,
            retry_guidance: None,
        }
    }

    pub fn tool_failure(
        tool_error_kind: impl Into<String>,
        status_text: impl Into<String>,
        retry_guidance: impl Into<String>,
    ) -> Self {
        Self {
            error_kind: "tool_execution_failed".to_string(),
            status_text: status_text.into(),
            tool_error_kind: Some(tool_error_kind.into()),
            provider_error_kind: Some("not_applicable".to_string()),
            retry_guidance: Some(retry_guidance.into()),
        }
    }

    pub fn provider_timeout(status_text: impl Into<String>) -> Self {
        Self {
            error_kind: "provider_timeout".to_string(),
            status_text: status_text.into(),
            tool_error_kind: None,
            provider_error_kind: Some("provider_progress_timeout".to_string()),
            retry_guidance: Some(
                "Retry the turn. Polar cancelled the timed-out attempt and will use a fresh fenced allocation."
                    .to_string(),
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InhouseAgentTurnOutput {
    pub session_id: String,
    pub reply_text: String,
    #[serde(default, skip_serializing_if = "InhouseTurnArtifacts::is_empty")]
    pub artifacts: InhouseTurnArtifacts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incomplete: Option<InhouseIncompleteTurn>,
}

impl InhouseAgentTurnOutput {
    pub fn success(session_id: impl Into<String>, reply_text: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            reply_text: reply_text.into(),
            artifacts: InhouseTurnArtifacts::default(),
            incomplete: None,
        }
    }

    pub fn with_artifacts(
        session_id: impl Into<String>,
        reply_text: impl Into<String>,
        artifacts: InhouseTurnArtifacts,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            reply_text: reply_text.into(),
            artifacts,
            incomplete: None,
        }
    }

    pub fn incomplete(
        session_id: impl Into<String>,
        reply_text: impl Into<String>,
        artifacts: InhouseTurnArtifacts,
        incomplete: InhouseIncompleteTurn,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            reply_text: reply_text.into(),
            artifacts,
            incomplete: Some(incomplete),
        }
    }
}

/// One backend-authoritative model and its ordered thinking controls.
///
/// Thinking values are opaque wire strings. Keeping them as strings preserves
/// provider extensions and the order advertised by native backend protocols.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseBackendModel {
    pub id: String,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thinking_levels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_thinking_level: Option<String>,
}

/// Backend-authoritative models that may be selected for subsequent turns.
///
/// `source` distinguishes a live endpoint or native backend protocol from a
/// configuration-only answer. `endpoint` is display-safe provenance such as
/// an HTTP `/models` URL or a protocol operation name; it must never contain
/// credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InhouseBackendModelCatalog {
    pub models: Vec<InhouseBackendModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

#[async_trait::async_trait]
pub trait InhouseAgentRunner: Send {
    /// Discover selectable models through the backend's authoritative
    /// protocol. `None` means the backend has no model-selection surface.
    /// Discovery errors must remain explicit; callers must not replace them
    /// with static provider tables that only look authoritative.
    async fn discover_model_catalog(
        &mut self,
    ) -> anyhow::Result<Option<InhouseBackendModelCatalog>> {
        Ok(None)
    }

    /// Read backend-native session state through the backend's supported
    /// protocol. `None` means that this backend does not expose inspection;
    /// callers must not reconstruct an answer from Polar state.
    async fn inspect_session(
        &mut self,
        _session_id: &str,
        _include_turns: bool,
    ) -> anyhow::Result<Option<Value>> {
        Ok(None)
    }

    async fn preflight_context_budget(
        &mut self,
        _input: &str,
        _turn_options: &InhouseTurnOptions,
        _configured_context_tokens: i64,
        _output_reserve_tokens: i64,
    ) -> anyhow::Result<Option<InhouseBackendContextBudget>> {
        Ok(None)
    }

    async fn run_turn(
        &mut self,
        existing_session_id: Option<String>,
        input: String,
        turn_options: InhouseTurnOptions,
        dynamic_tool_handler: Option<Arc<dyn InhouseDynamicToolHandler>>,
    ) -> anyhow::Result<(String, String)>;

    async fn run_turn_streaming(
        &mut self,
        existing_session_id: Option<String>,
        input: String,
        turn_options: InhouseTurnOptions,
        event_handler: Option<Box<dyn InhouseTurnEventHandler + Send>>,
        turn_control: Option<Arc<InhouseTurnControl>>,
        dynamic_tool_handler: Option<Arc<dyn InhouseDynamicToolHandler>>,
    ) -> anyhow::Result<(String, String)> {
        let _ = event_handler;
        let _ = turn_control;
        self.run_turn(
            existing_session_id,
            input,
            turn_options,
            dynamic_tool_handler,
        )
        .await
    }

    async fn run_turn_outcome(
        &mut self,
        existing_session_id: Option<String>,
        input: String,
        turn_options: InhouseTurnOptions,
        dynamic_tool_handler: Option<Arc<dyn InhouseDynamicToolHandler>>,
    ) -> anyhow::Result<InhouseAgentTurnOutcome> {
        let (session_id, reply_text) = self
            .run_turn(
                existing_session_id,
                input,
                turn_options,
                dynamic_tool_handler,
            )
            .await?;
        Ok(InhouseAgentTurnOutcome::ImmediateOutput(
            InhouseAgentTurnOutput::success(session_id, reply_text),
        ))
    }

    async fn run_turn_streaming_outcome(
        &mut self,
        existing_session_id: Option<String>,
        input: String,
        turn_options: InhouseTurnOptions,
        event_handler: Option<Box<dyn InhouseTurnEventHandler + Send>>,
        turn_control: Option<Arc<InhouseTurnControl>>,
        dynamic_tool_handler: Option<Arc<dyn InhouseDynamicToolHandler>>,
    ) -> anyhow::Result<InhouseAgentTurnOutcome> {
        let (session_id, reply_text) = self
            .run_turn_streaming(
                existing_session_id,
                input,
                turn_options,
                event_handler,
                turn_control,
                dynamic_tool_handler,
            )
            .await?;
        Ok(InhouseAgentTurnOutcome::ImmediateOutput(
            InhouseAgentTurnOutput::success(session_id, reply_text),
        ))
    }
}

pub trait InhouseRunnerFactory: Send + Sync {
    fn build_tool_free_runner(&self, input: InhouseRunnerBuildInput<'_>) -> anyhow::Result<Box<dyn InhouseAgentRunner + Send>> {
        self.build_memory_runner(input)
    }
    /// Isolated text generation with all native and dynamic tools disabled.
    fn build_memory_runner(
        &self,
        _input: InhouseRunnerBuildInput<'_>,
    ) -> anyhow::Result<Box<dyn InhouseAgentRunner + Send>> {
        anyhow::bail!("tool-free memory extraction is not supported by this runner factory")
    }

    fn build_runner(
        &self,
        input: InhouseRunnerBuildInput<'_>,
    ) -> anyhow::Result<Box<dyn InhouseAgentRunner + Send>>;
}

#[derive(Debug, Clone, Copy)]
pub struct InhouseRunnerBuildInput<'a> {
    pub inhouse_polar: &'a polar_inhouse_model::InhousePolarRecord,
    pub backend_profile: &'a polar_inhouse_model::InhouseBackendProfileRecord,
}

impl<F> InhouseRunnerFactory for F
where
    F: Fn(InhouseRunnerBuildInput<'_>) -> anyhow::Result<Box<dyn InhouseAgentRunner + Send>>
        + Send
        + Sync,
{
    fn build_runner(
        &self,
        input: InhouseRunnerBuildInput<'_>,
    ) -> anyhow::Result<Box<dyn InhouseAgentRunner + Send>> {
        self(input)
    }
}



mod draft_refine;
pub use draft_refine::{DraftRefineRequest, DraftRefineResult};
