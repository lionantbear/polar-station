use crate::causal_identity::{
    adam_tool_arguments_hash, adam_tool_invocation_id, AdamCausalIdentityMode,
    AdamToolInvocationEvidence, AdamToolInvocationStatus,
};
use crate::config::{
    AdamAgentConfig, AdamConnectionConfig, AdamLlmConnectionMode, LlamaTransport, OpenAiConfig,
};
use crate::engine_manager::{
    normalize_connector_url, AdamCachePlan, AdamEngineAllocation, AdamEngineAllocationWaitConfig,
    AdamEngineControlIntent, AdamEngineTurnPriority, AdamInferenceEngineManager,
};
use crate::llama::{
    build_chat_completions_request, build_responses_request, parse_chat_completions_response,
    parse_responses_response, LlamaCppSlotsClient, LlamaWireProjection,
};
use crate::openai::{
    build_openai_headers, build_openai_response_create_message, openai_responses_websocket_url,
    parse_openai_event, AdamOAuthCredential, OpenAiResponsesEvent,
};
use crate::projection_store::{
    AdamProviderUsageCounters, AdamProviderUsageTelemetry, AdamRenderProjection,
    AdamRuntimeProjectionStore, AdamSessionBinding, AdamSessionBindingKey, AdamToolCallCorrelation,
    AdamTurnAttempt, AdamTurnAttemptStatus, AdamUsageAccountingSource,
};
use crate::tool_bridge::{
    adam_provider_tool_schemas, execute_adam_tool_call, normalize_adam_provider_tool_call,
};
use anyhow::Context;
use async_trait::async_trait;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use polar_inhouse_contract::{
    InhouseAgentRunner, InhouseAgentTurnOutcome, InhouseAgentTurnOutput, InhouseBackendBusyError,
    InhouseBackendCapabilityNegotiation, InhouseBackendTurnRequest, InhouseCanonicalThreadMessage,
    InhouseDynamicToolHandler, InhouseIncompleteTurn, InhousePromptLayer, InhousePromptSegment,
    InhouseProviderCacheEvidence, InhouseToolPlan, InhouseTurnArtifacts, InhouseTurnControl,
    InhouseTurnEventHandler, InhouseTurnOptions, InhouseTurnSchedulingPriority,
    PolarPromptProjection,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdamBusyKind {
    LlamaProviderBusy,
    OpenAiProviderBusy,
}

impl AdamBusyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LlamaProviderBusy => "llama_provider_busy",
            Self::OpenAiProviderBusy => "openai_provider_busy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdamBackendError {
    Auth(String),
    Transport(String),
    Parse(String),
    Provider {
        status: Option<u16>,
        message: String,
    },
    ProviderBusy {
        kind: AdamBusyKind,
        message: String,
    },
    MalformedToolEnvelope(String),
    ToolLoopLimit {
        max_tool_iterations: usize,
    },
}

impl AdamBackendError {
    fn is_progress_timeout(&self) -> bool {
        matches!(self, Self::Transport(message) if message.contains("made no progress"))
    }

    pub fn into_anyhow(self) -> anyhow::Error {
        match self {
            Self::ProviderBusy { kind, message } => {
                InhouseBackendBusyError::new(kind.as_str(), message).into()
            }
            other => anyhow::anyhow!(other),
        }
    }
}

impl fmt::Display for AdamBackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auth(message) => write!(f, "Adam auth error: {message}"),
            Self::Transport(message) => write!(f, "Adam transport error: {message}"),
            Self::Parse(message) => write!(f, "Adam parse error: {message}"),
            Self::Provider { status, message } => {
                if let Some(status) = status {
                    write!(f, "Adam provider returned HTTP {status}: {message}")
                } else {
                    write!(f, "Adam provider error: {message}")
                }
            }
            Self::ProviderBusy { kind, message } => {
                write!(f, "Adam provider busy {}: {message}", kind.as_str())
            }
            Self::MalformedToolEnvelope(message) => {
                write!(f, "Adam malformed tool envelope: {message}")
            }
            Self::ToolLoopLimit {
                max_tool_iterations,
            } => write!(
                f,
                "Adam tool loop exceeded max_tool_iterations={max_tool_iterations}"
            ),
        }
    }
}

impl std::error::Error for AdamBackendError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamTurnContext {
    pub owner_id: String,
    pub thread_id: String,
    pub inhouse_polar_id: String,
    pub backend_profile_id: String,
    pub request_message_id: String,
}

impl AdamTurnContext {
    pub fn from_request(request: &InhouseBackendTurnRequest) -> Self {
        let turn_ref = &request.turn_ref;
        Self {
            owner_id: turn_ref.owner_id.clone(),
            thread_id: turn_ref.thread_id.clone(),
            inhouse_polar_id: turn_ref.inhouse_polar_id.clone(),
            backend_profile_id: turn_ref.backend_profile_id.clone(),
            request_message_id: turn_ref.request_message_id.clone(),
        }
    }

    pub fn stable_session_id(&self) -> String {
        format!(
            "adam:{}:{}:{}:{}:{}",
            self.owner_id,
            self.thread_id,
            self.inhouse_polar_id,
            self.backend_profile_id,
            self.request_message_id
        )
    }

    pub fn binding_key(&self) -> AdamSessionBindingKey {
        AdamSessionBindingKey {
            owner_id: self.owner_id.clone(),
            thread_id: self.thread_id.clone(),
            inhouse_polar_id: self.inhouse_polar_id.clone(),
            backend_profile_id: self.backend_profile_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdamToolCall {
    /// Model-visible identifier used to pair the provider tool result.
    pub call_id: String,
    /// Raw provider identifier, absent when a compatibility parser supplied a wire fallback.
    pub provider_call_id: Option<String>,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdamToolResult {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
    pub output_text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdamProviderRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_messages: Option<Vec<InhouseCanonicalThreadMessage>>,
    pub session_id: String,
    pub turn_context: AdamTurnContext,
    pub instructions: String,
    pub stable_prefix: Option<String>,
    pub input: String,
    pub tools: Vec<Value>,
    pub tool_results: Vec<AdamToolResult>,
    /// Opaque Responses output and result items for this turn's tool loop only.
    /// Excluded from durable request projections, including encrypted reasoning.
    #[serde(skip)]
    pub responses_items: Vec<Value>,
    pub prompt_cache_key: Option<String>,
    pub provider_slot_id: Option<i64>,
    pub max_output_tokens: Option<i64>,
    /// Exact one-turn native llama wire projection. This transient payload is
    /// never serialized into engine evidence or durable projection metadata.
    #[serde(skip)]
    pub wire_projection: Option<LlamaWireProjection>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdamProviderTurnEnd {
    pub provider_response_id: Option<String>,
    pub usage: Option<Value>,
    pub cache_evidence: Option<InhouseProviderCacheEvidence>,
    pub raw: Option<Value>,
}

pub type AdamTurnEventSink = Arc<Mutex<Box<dyn InhouseTurnEventHandler + Send>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdamThinkingSource {
    OpenAiReasoningTextDelta,
    OpenAiReasoningSummaryTextDelta,
    OpenAiReasoningOutputItem,
    LlamaThinkTag,
    OpenAiCompatibleReasoning,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdamThinkingBlock {
    pub id: String,
    pub source: AdamThinkingSource,
    pub phase: String,
    pub text: String,
    pub closed: bool,
    pub visibility: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redaction: Option<AdamThinkingRedaction>,
}

/// Safe metadata emitted when Adam suppresses provider thinking that reproduces
/// a protected compiled instruction. The original text is deliberately not
/// retained in LGP-visible artifacts or stream events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamThinkingRedaction {
    pub reason: String,
    pub count: u32,
}

const ADAM_PROTECTED_THINKING_WINDOW_CHARS: usize = 24;
const ADAM_PROTECTED_THINKING_MIN_CHARS: usize = 8;
const ADAM_PROTECTED_THINKING_MARKER: &str = "[Protected provider thinking redacted]";

#[derive(Debug, Clone)]
struct AdamActiveThinkingRedaction {
    source_index: usize,
    next_char_index: usize,
}

#[derive(Debug, Clone, Default)]
struct AdamSanitizedThinkingDelta {
    text: String,
    redaction_count: u32,
}

/// Streaming guard for provider-emitted reasoning. It holds at most one
/// protected-literal window behind the live stream so a secret split across
/// provider chunks is never published before it can be recognised.
#[derive(Debug, Clone)]
struct AdamThinkingDisclosureGate {
    protected_sources: Vec<Vec<char>>,
    pending: Vec<char>,
    active_redaction: Option<AdamActiveThinkingRedaction>,
}

impl AdamThinkingDisclosureGate {
    fn from_compiled_instructions(instructions: &str) -> Option<Self> {
        let mut protected_sources = Vec::<Vec<char>>::new();
        let mut push_source = |source: &str| {
            let chars = source.trim().chars().collect::<Vec<_>>();
            if chars.len() >= ADAM_PROTECTED_THINKING_MIN_CHARS
                && !protected_sources.iter().any(|existing| existing == &chars)
            {
                protected_sources.push(chars);
            }
        };
        push_source(instructions);
        for line in instructions.lines() {
            push_source(line);
        }
        (!protected_sources.is_empty()).then_some(Self {
            protected_sources,
            pending: Vec::new(),
            active_redaction: None,
        })
    }

    fn push(&mut self, text: &str, closed: bool) -> AdamSanitizedThinkingDelta {
        self.pending.extend(text.chars());
        self.drain(closed)
    }

    fn finish(&mut self) -> AdamSanitizedThinkingDelta {
        self.drain(true)
    }

    fn drain(&mut self, closed: bool) -> AdamSanitizedThinkingDelta {
        let mut output = AdamSanitizedThinkingDelta::default();
        loop {
            if self.consume_active_redaction() {
                continue;
            }
            if let Some((pending_start, source_index, source_start, window_len)) =
                self.find_protected_match()
            {
                output.text.extend(self.pending.drain(..pending_start));
                self.pending.drain(..window_len);
                output.text.push_str(ADAM_PROTECTED_THINKING_MARKER);
                output.redaction_count = output.redaction_count.saturating_add(1);
                self.active_redaction = Some(AdamActiveThinkingRedaction {
                    source_index,
                    next_char_index: source_start.saturating_add(window_len),
                });
                continue;
            }

            let releasable = if closed {
                self.pending.len()
            } else {
                self.pending
                    .len()
                    .saturating_sub(self.retained_tail_chars())
            };
            if releasable == 0 {
                break;
            }
            output.text.extend(self.pending.drain(..releasable));
            break;
        }
        output
    }

    /// Returns true when a redaction consumed text or completed, so the caller
    /// should rescan the pending stream before releasing any bytes.
    fn consume_active_redaction(&mut self) -> bool {
        let Some(active) = self.active_redaction.as_mut() else {
            return false;
        };
        let source = &self.protected_sources[active.source_index];
        let mut consumed = false;
        while active.next_char_index < source.len()
            && self
                .pending
                .first()
                .is_some_and(|next| *next == source[active.next_char_index])
        {
            self.pending.remove(0);
            active.next_char_index += 1;
            consumed = true;
        }
        let completed = active.next_char_index >= source.len();
        let mismatched = !self.pending.is_empty() && !completed;
        if completed || mismatched {
            self.active_redaction = None;
            return true;
        }
        consumed
    }

    fn find_protected_match(&self) -> Option<(usize, usize, usize, usize)> {
        let mut best: Option<(usize, usize, usize, usize)> = None;
        for pending_start in 0..self.pending.len() {
            for (source_index, source) in self.protected_sources.iter().enumerate() {
                let window_len = source.len().min(ADAM_PROTECTED_THINKING_WINDOW_CHARS);
                if window_len < ADAM_PROTECTED_THINKING_MIN_CHARS
                    || pending_start.saturating_add(window_len) > self.pending.len()
                {
                    continue;
                }
                for source_start in 0..=source.len().saturating_sub(window_len) {
                    if self.pending[pending_start..pending_start + window_len]
                        == source[source_start..source_start + window_len]
                    {
                        let candidate = (pending_start, source_index, source_start, window_len);
                        if best.as_ref().is_none_or(|current| candidate.0 < current.0) {
                            best = Some(candidate);
                        }
                        break;
                    }
                }
            }
            if best.is_some_and(|current| current.0 == pending_start) {
                break;
            }
        }
        best
    }

    fn retained_tail_chars(&self) -> usize {
        self.protected_sources
            .iter()
            .map(|source| {
                source
                    .len()
                    .min(ADAM_PROTECTED_THINKING_WINDOW_CHARS)
                    .saturating_sub(1)
            })
            .max()
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AdamProviderEvent {
    AnswerDelta(String),
    ThinkingDelta {
        text: String,
        source: AdamThinkingSource,
        closed: Option<bool>,
    },
    ToolCall(AdamToolCall),
    RawOutputItem(Value),
}

#[async_trait]
pub trait AdamProviderEventSink: Send {
    async fn emit(&mut self, event: AdamProviderEvent) -> Result<(), AdamBackendError>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdamAccumulatedTurn {
    pub session_id: String,
    pub text: String,
    pub tool_calls: Vec<AdamToolCall>,
    pub provider_response_id: Option<String>,
    pub usage: Option<Value>,
    pub cache_evidence: Option<InhouseProviderCacheEvidence>,
    pub raw: Option<Value>,
    pub artifacts: InhouseTurnArtifacts,
    pub no_answer_after_thinking: bool,
}

pub struct AdamTurnAccumulator {
    event_sink: Option<AdamTurnEventSink>,
    answer_text: String,
    tool_calls: Vec<AdamToolCall>,
    raw_output_items: Vec<Value>,
    thinking_blocks: Vec<AdamThinkingBlock>,
    open_thinking_block: Option<usize>,
    thinking_disclosure_gate: Option<AdamThinkingDisclosureGate>,
    thinking_disclosure_source: Option<AdamThinkingSource>,
}

impl AdamTurnAccumulator {
    pub fn new(event_sink: Option<AdamTurnEventSink>) -> Self {
        Self {
            event_sink,
            answer_text: String::new(),
            tool_calls: Vec::new(),
            raw_output_items: Vec::new(),
            thinking_blocks: Vec::new(),
            open_thinking_block: None,
            thinking_disclosure_gate: None,
            thinking_disclosure_source: None,
        }
    }

    /// Apply the compiled-instruction disclosure gate to thinking only. Answer
    /// deltas remain provider text; no raw compiled instruction is added to the
    /// user-facing answer path by this constructor.
    pub fn with_protected_instructions(
        event_sink: Option<AdamTurnEventSink>,
        compiled_instructions: &str,
    ) -> Self {
        let mut accumulator = Self::new(event_sink);
        accumulator.thinking_disclosure_gate =
            AdamThinkingDisclosureGate::from_compiled_instructions(compiled_instructions);
        accumulator
    }

    pub fn finish(mut self, session_id: String, end: AdamProviderTurnEnd) -> AdamAccumulatedTurn {
        self.flush_pending_thinking_for_finish();
        for block in &mut self.thinking_blocks {
            if block.text.trim().is_empty() {
                block.closed = true;
            }
        }
        let thinking_values = self
            .thinking_blocks
            .into_iter()
            .filter(|block| !block.text.trim().is_empty())
            .map(|block| serde_json::to_value(block).unwrap_or_else(|_| json!({})))
            .collect::<Vec<_>>();
        let no_answer_after_thinking =
            self.answer_text.trim().is_empty() && !thinking_values.is_empty();
        let raw = match (end.raw, self.raw_output_items.is_empty()) {
            (Some(raw), true) => Some(raw),
            (Some(raw), false) => Some(json!({
                "provider": raw,
                "raw_output_items": self.raw_output_items,
            })),
            (None, false) => Some(json!({ "raw_output_items": self.raw_output_items })),
            (None, true) => None,
        };
        AdamAccumulatedTurn {
            session_id,
            text: self.answer_text,
            tool_calls: self.tool_calls,
            provider_response_id: end.provider_response_id,
            usage: end.usage,
            cache_evidence: end.cache_evidence.clone(),
            raw,
            artifacts: InhouseTurnArtifacts {
                thinking_blocks: thinking_values,
                cache_evidence: end.cache_evidence.iter().cloned().collect(),
                prompt_parity_trace: None,
                prompt_adapter_receipt: None,
            },
            no_answer_after_thinking,
        }
    }

    fn append_thinking(
        &mut self,
        text: String,
        source: AdamThinkingSource,
        closed: Option<bool>,
    ) -> Result<(), AdamBackendError> {
        if self.thinking_disclosure_gate.is_none() {
            return self.append_sanitized_thinking(text, source, closed, 0);
        }

        if let Some(previous_source) = self
            .thinking_disclosure_source
            .filter(|previous_source| *previous_source != source)
        {
            let pending = self
                .thinking_disclosure_gate
                .as_mut()
                .expect("gate checked above")
                .finish();
            self.append_sanitized_thinking(
                pending.text,
                previous_source,
                Some(false),
                pending.redaction_count,
            )?;
            self.thinking_disclosure_source = None;
        }

        let sanitized = self
            .thinking_disclosure_gate
            .as_mut()
            .expect("gate checked above")
            .push(&text, closed.is_some());
        self.append_sanitized_thinking(sanitized.text, source, closed, sanitized.redaction_count)?;
        self.thinking_disclosure_source = if closed.is_some() { None } else { Some(source) };
        Ok(())
    }

    fn flush_pending_thinking_for_finish(&mut self) {
        let Some(source) = self.thinking_disclosure_source.take() else {
            return;
        };
        let Some(gate) = self.thinking_disclosure_gate.as_mut() else {
            return;
        };
        let pending = gate.finish();
        self.record_sanitized_thinking(pending.text, source, Some(false), pending.redaction_count);
    }

    fn append_sanitized_thinking(
        &mut self,
        text: String,
        source: AdamThinkingSource,
        closed: Option<bool>,
        redaction_count: u32,
    ) -> Result<(), AdamBackendError> {
        let index = self.ensure_thinking_block(source);
        self.record_thinking_redaction(index, redaction_count);

        if !text.is_empty() {
            if let Some(sink) = &self.event_sink {
                with_adam_event_sink(sink, |handler| {
                    handler.on_intermediate_status_delta(&text, "thinking")
                })?;
            }
        }
        self.record_sanitized_thinking_at_index(index, text, closed);
        Ok(())
    }

    fn record_sanitized_thinking(
        &mut self,
        text: String,
        source: AdamThinkingSource,
        closed: Option<bool>,
        redaction_count: u32,
    ) {
        let index = self.ensure_thinking_block(source);
        self.record_thinking_redaction(index, redaction_count);
        self.record_sanitized_thinking_at_index(index, text, closed);
    }

    fn ensure_thinking_block(&mut self, source: AdamThinkingSource) -> usize {
        match self.open_thinking_block {
            Some(index)
                if self
                    .thinking_blocks
                    .get(index)
                    .is_some_and(|block| block.source == source && !block.closed) =>
            {
                index
            }
            _ => {
                let index = self.thinking_blocks.len();
                self.thinking_blocks.push(AdamThinkingBlock {
                    id: format!("thinking-{index}"),
                    source,
                    phase: "thinking".to_string(),
                    text: String::new(),
                    closed: false,
                    visibility: "operator_debug".to_string(),
                    redaction: None,
                });
                self.open_thinking_block = Some(index);
                index
            }
        }
    }

    fn record_thinking_redaction(&mut self, index: usize, redaction_count: u32) {
        if redaction_count > 0 {
            if let Some(block) = self.thinking_blocks.get_mut(index) {
                let redaction = block.redaction.get_or_insert(AdamThinkingRedaction {
                    reason: "compiled_instruction".to_string(),
                    count: 0,
                });
                redaction.count = redaction.count.saturating_add(redaction_count);
            }
        }
    }

    fn record_sanitized_thinking_at_index(
        &mut self,
        index: usize,
        text: String,
        closed: Option<bool>,
    ) {
        if !text.is_empty() {
            if let Some(block) = self.thinking_blocks.get_mut(index) {
                block.text.push_str(&text);
            }
        }
        if let Some(closed) = closed {
            if let Some(block) = self.thinking_blocks.get_mut(index) {
                block.closed = closed;
            }
            self.open_thinking_block = None;
        }
    }
}

#[async_trait]
impl AdamProviderEventSink for AdamTurnAccumulator {
    async fn emit(&mut self, event: AdamProviderEvent) -> Result<(), AdamBackendError> {
        match event {
            AdamProviderEvent::AnswerDelta(text) => {
                if !text.is_empty() {
                    if let Some(sink) = &self.event_sink {
                        with_adam_event_sink(sink, |handler| handler.on_agent_text_delta(&text))?;
                    }
                    self.answer_text.push_str(&text);
                }
            }
            AdamProviderEvent::ThinkingDelta {
                text,
                source,
                closed,
            } => self.append_thinking(text, source, closed)?,
            AdamProviderEvent::ToolCall(call) => self.tool_calls.push(call),
            AdamProviderEvent::RawOutputItem(item) => self.raw_output_items.push(item),
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamRunnerProjectionHashes {
    pub source_digest: String,
    pub renderer_version: String,
    pub template_hash: String,
    pub exact_prefix_hash: String,
    pub input_hash: String,
    pub tool_schema_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdamRunnerHashInput {
    pub source_refs: Vec<String>,
    pub renderer_version: String,
    pub template: String,
    pub exact_prefix: String,
    pub input: String,
    pub tools: Vec<Value>,
}

/// Adam wire-prefix projection of the shared backend request. Compaction uses
/// this same renderer before physical preparation, so a prepared slot cannot
/// warm bytes that differ from the next Adam turn's stable prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamPreparedCompactProjection {
    pub exact_prefix_payload: String,
    pub full_prompt_payload: String,
    pub has_tools: bool,
    pub hashes: AdamRunnerProjectionHashes,
}

pub fn project_adam_prepared_compact_request(
    request: &polar_inhouse_contract::InhouseBackendTurnRequest,
) -> AdamPreparedCompactProjection {
    let instruction_layers = [
        InhousePromptLayer::System,
        InhousePromptLayer::Developer,
        InhousePromptLayer::ToolUsageProtocol,
        InhousePromptLayer::UserProjectContext,
    ];
    let instructions = request
        .prompt_envelope
        .blocks
        .iter()
        .filter(|block| instruction_layers.contains(&block.layer))
        .map(|block| format!("## {}\n{}", block.block_id, block.body_text.trim()))
        .collect::<Vec<_>>()
        .join("\n\n");
    let projection = PolarPromptProjection::from_segments(
        &instructions,
        request.message_projection.segments.clone(),
        request
            .message_projection
            .current_turn_message_id
            .as_deref(),
    );
    let source_refs = request
        .prompt_envelope
        .blocks
        .iter()
        .map(|block| block.source_ref.clone())
        .chain(
            request
                .message_projection
                .segments
                .iter()
                .map(|segment| segment.source_ref.clone()),
        )
        .collect::<Vec<_>>();
    let tools = adam_provider_tool_schemas(&request.tool_plan);
    let has_tools = !tools.is_empty();
    let full_prompt_payload = projection.full_prompt.clone();
    let hashes = compute_adam_runner_projection_hashes(&AdamRunnerHashInput {
        source_refs,
        renderer_version: ADAM_RENDERER_VERSION.to_string(),
        template: instructions,
        exact_prefix: projection.rendered_prompt.clone(),
        input: full_prompt_payload.clone(),
        tools,
    });
    AdamPreparedCompactProjection {
        exact_prefix_payload: projection.rendered_prompt,
        full_prompt_payload,
        has_tools,
        hashes,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamCompactionEpochInvalidation {
    pub previous_context_compaction_epoch: Option<i64>,
    pub new_context_compaction_epoch: i64,
    pub render_projections_invalidated: usize,
    pub cache_epochs_invalidated: usize,
}

pub async fn invalidate_adam_context_compaction_epoch(
    store: &dyn AdamRuntimeProjectionStore,
    session_id: &str,
    new_context_compaction_epoch: i64,
) -> anyhow::Result<AdamCompactionEpochInvalidation> {
    let previous_context_compaction_epoch =
        store.latest_context_compaction_epoch(session_id).await?;
    if previous_context_compaction_epoch
        .is_some_and(|previous| new_context_compaction_epoch < previous)
    {
        anyhow::bail!(
            "Adam context compaction epoch regressed from {} to {}",
            previous_context_compaction_epoch.unwrap_or_default(),
            new_context_compaction_epoch
        );
    }
    let changed = previous_context_compaction_epoch
        .is_some_and(|previous| previous != new_context_compaction_epoch);
    let (render_projections_invalidated, cache_epochs_invalidated) = if changed {
        (
            store
                .invalidate_render_projections(session_id, "context_compacted")
                .await?,
            store
                .invalidate_cache_epochs(session_id, "context_compacted")
                .await?,
        )
    } else {
        (0, 0)
    };
    Ok(AdamCompactionEpochInvalidation {
        previous_context_compaction_epoch,
        new_context_compaction_epoch,
        render_projections_invalidated,
        cache_epochs_invalidated,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdamQueuedWorkKind {
    UserInput,
    ToolFollowUp,
    Compact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdamQueueDecision {
    RunImmediately,
    QueueBehindActiveTurn,
    InterruptActiveTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamQueueMode {
    pub active_turn: bool,
    pub explicit_interrupt: bool,
}

pub fn compute_adam_runner_projection_hashes(
    input: &AdamRunnerHashInput,
) -> AdamRunnerProjectionHashes {
    AdamRunnerProjectionHashes {
        source_digest: stable_hash_json(&input.source_refs),
        renderer_version: input.renderer_version.clone(),
        template_hash: stable_hash_str(&input.template),
        exact_prefix_hash: stable_hash_str(&input.exact_prefix),
        input_hash: stable_hash_str(&input.input),
        tool_schema_hash: stable_hash_json(&input.tools),
    }
}

pub fn adam_queue_decision(_kind: AdamQueuedWorkKind, mode: AdamQueueMode) -> AdamQueueDecision {
    match (mode.active_turn, mode.explicit_interrupt) {
        (false, _) => AdamQueueDecision::RunImmediately,
        (true, true) => AdamQueueDecision::InterruptActiveTurn,
        (true, false) => AdamQueueDecision::QueueBehindActiveTurn,
    }
}

fn stable_hash_str(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize())
}

fn stable_hash_json<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize())
}

const ADAM_RENDERER_VERSION: &str = "adam-renderer-v1";
const ADAM_CONTEXT_COMPACTION_EPOCH_UNKNOWN: i64 = 0;
const ADAM_LEGACY_CANONICAL_MESSAGE_REVISION: &str = "rev-1";

fn adam_projection_source_refs(
    context: &AdamTurnContext,
    options: &InhouseTurnOptions,
) -> Vec<String> {
    if let Some(request) = options.backend_turn_request.as_ref() {
        let mut refs = request
            .prompt_envelope
            .blocks
            .iter()
            .map(|block| block.source_ref.clone())
            .chain(
                request
                    .message_projection
                    .segments
                    .iter()
                    .map(|segment| segment.source_ref.clone()),
            )
            .filter(|source_ref| !source_ref.trim().is_empty())
            .collect::<Vec<_>>();
        refs.dedup();
        if !refs.is_empty() {
            return refs;
        }
    }
    let mut refs = options
        .canonical_thread_messages
        .iter()
        .map(|message| format!("thread_message:{}", message.message_id))
        .collect::<Vec<_>>();
    if refs.is_empty() {
        refs.push(format!("thread_message:{}", context.request_message_id));
    }
    refs.dedup();
    refs
}

fn adam_context_compaction_epoch(options: &InhouseTurnOptions) -> i64 {
    options
        .backend_turn_request
        .as_ref()
        .map(|request| {
            request
                .message_projection
                .segments
                .iter()
                .map(|segment| segment.compaction_epoch)
                .max()
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

fn ensure_adam_capability_negotiation(options: &InhouseTurnOptions) -> anyhow::Result<()> {
    let Some(request) = options.backend_turn_request.as_ref() else {
        anyhow::bail!(
            "Adam backend requires an assembled InhouseBackendTurnRequest; legacy options must be adapted above the runner boundary"
        );
    };
    let negotiation = InhouseBackendCapabilityNegotiation::negotiate(
        request.requested_capabilities,
        request.effective_capabilities,
    );
    if !negotiation.satisfied() {
        anyhow::bail!(
            "Adam backend capability negotiation failed before execution; unsupported requested capabilities: {}",
            negotiation.missing_required_capabilities.join(", ")
        );
    }
    Ok(())
}

fn adam_engine_turn_priority(options: &InhouseTurnOptions) -> AdamEngineTurnPriority {
    let priority = options
        .backend_turn_request
        .as_ref()
        .map(|request| request.scheduling_priority)
        .unwrap_or(options.scheduling_priority);
    match priority {
        InhouseTurnSchedulingPriority::ActiveThread => AdamEngineTurnPriority::ActiveThread,
        InhouseTurnSchedulingPriority::Background => AdamEngineTurnPriority::Background,
    }
}

fn adam_tool_plan(
    context: &AdamTurnContext,
    options: &InhouseTurnOptions,
    dynamic_tool_handler: Option<&Arc<dyn InhouseDynamicToolHandler>>,
) -> InhouseToolPlan {
    if let Some(request) = options.backend_turn_request.as_ref() {
        return request.tool_plan.clone();
    }
    let _ = (context, dynamic_tool_handler);
    unreachable!("Adam capability negotiation requires a shared backend request")
}

fn adam_render_projection_id(
    session_id: &str,
    request_message_id: &str,
    hashes: &AdamRunnerProjectionHashes,
) -> String {
    format!(
        "adam_projection_{}",
        stable_hash_json(&json!({
            "session_id": session_id,
            "request_message_id": request_message_id,
            "source_digest": hashes.source_digest,
            "renderer_version": hashes.renderer_version,
            "template_hash": hashes.template_hash,
            "tool_schema_hash": hashes.tool_schema_hash,
            "exact_prefix_hash": hashes.exact_prefix_hash,
            "input_hash": hashes.input_hash,
        }))
    )
}

fn adam_turn_attempt_id(context: &AdamTurnContext, iteration: usize) -> String {
    format!(
        "adam_attempt_{}",
        stable_hash_json(&json!({
            "owner_id": context.owner_id,
            "thread_id": context.thread_id,
            "inhouse_polar_id": context.inhouse_polar_id,
            "backend_profile_id": context.backend_profile_id,
            "request_message_id": context.request_message_id,
            "iteration": iteration,
        }))
    )
}

fn adam_transport_label(config: &AdamAgentConfig) -> Option<String> {
    match config.backend.connection_mode() {
        AdamLlmConnectionMode::OpenAi => Some("websocket".to_string()),
        AdamLlmConnectionMode::Llama => config
            .backend
            .llama()
            .map(|llama| llama.transport.as_str().to_string()),
    }
}

fn adam_usage_i64(value: &Value, pointers: &[&str]) -> i64 {
    pointers
        .iter()
        .find_map(|pointer| {
            value.pointer(pointer).and_then(Value::as_i64).or_else(|| {
                value
                    .pointer(pointer)
                    .and_then(Value::as_u64)
                    .and_then(|raw| i64::try_from(raw).ok())
            })
        })
        .unwrap_or(0)
}

fn adam_usage_counters(
    mode: AdamLlmConnectionMode,
    usage: Option<&Value>,
    cache_evidence: Option<&InhouseProviderCacheEvidence>,
) -> AdamProviderUsageCounters {
    if let Some(evidence) = cache_evidence {
        let output = evidence.output_tokens.unwrap_or_else(|| {
            usage
                .map(|value| {
                    adam_usage_i64(
                        value,
                        &[
                            "/output_tokens",
                            "/completion_tokens",
                            "/usage/output_tokens",
                            "/usage/completion_tokens",
                        ],
                    )
                })
                .unwrap_or(0)
        });
        return AdamProviderUsageCounters {
            input_uncached: evidence.input_uncached_tokens,
            cache_read: evidence.cache_read_tokens,
            cache_write: evidence.cache_write_tokens,
            output,
            total: evidence.input_uncached_tokens
                + evidence.cache_read_tokens
                + evidence.cache_write_tokens
                + output,
        };
    }
    match mode {
        AdamLlmConnectionMode::OpenAi => {
            let Some(usage) = usage else {
                return AdamProviderUsageCounters {
                    input_uncached: 0,
                    cache_read: 0,
                    cache_write: 0,
                    output: 0,
                    total: 0,
                };
            };
            let input = adam_usage_i64(
                usage,
                &[
                    "/input_tokens",
                    "/prompt_tokens",
                    "/usage/input_tokens",
                    "/usage/prompt_tokens",
                ],
            );
            let cache_read = adam_usage_i64(
                usage,
                &[
                    "/input_tokens_details/cached_tokens",
                    "/prompt_tokens_details/cached_tokens",
                    "/cache_read_input_tokens",
                    "/cached_tokens",
                ],
            );
            let cache_write = adam_usage_i64(
                usage,
                &[
                    "/input_tokens_details/cache_write_tokens",
                    "/prompt_tokens_details/cache_write_tokens",
                    "/input_tokens_details/cache_creation_input_tokens",
                    "/prompt_tokens_details/cache_creation_input_tokens",
                    "/cache_write_input_tokens",
                ],
            );
            let output = adam_usage_i64(
                usage,
                &[
                    "/output_tokens",
                    "/completion_tokens",
                    "/usage/output_tokens",
                    "/usage/completion_tokens",
                ],
            );
            let input_uncached = input.saturating_sub(cache_read).saturating_sub(cache_write);
            let reported_total = adam_usage_i64(usage, &["/total_tokens", "/usage/total_tokens"]);
            AdamProviderUsageCounters {
                input_uncached,
                cache_read,
                cache_write,
                output,
                total: reported_total.max(input_uncached + cache_read + cache_write + output),
            }
        }
        AdamLlmConnectionMode::Llama => {
            let cache_read = 0;
            let input_uncached = 0;
            AdamProviderUsageCounters {
                input_uncached,
                cache_read,
                cache_write: 0,
                output: usage
                    .map(|value| adam_usage_i64(value, &["/output_tokens", "/completion_tokens"]))
                    .unwrap_or(0),
                total: input_uncached + cache_read,
            }
        }
    }
}

fn openai_cache_evidence_from_usage(
    usage: Option<&Value>,
    wire_family: &str,
    dialect: &str,
) -> Option<InhouseProviderCacheEvidence> {
    let usage = usage?;
    let input = adam_usage_i64(
        usage,
        &[
            "/input_tokens",
            "/prompt_tokens",
            "/usage/input_tokens",
            "/usage/prompt_tokens",
        ],
    );
    let cache_read = adam_usage_i64(
        usage,
        &[
            "/input_tokens_details/cached_tokens",
            "/prompt_tokens_details/cached_tokens",
            "/cache_read_input_tokens",
            "/cached_tokens",
        ],
    );
    let cache_write = adam_usage_i64(
        usage,
        &[
            "/input_tokens_details/cache_write_tokens",
            "/prompt_tokens_details/cache_write_tokens",
            "/input_tokens_details/cache_creation_input_tokens",
            "/prompt_tokens_details/cache_creation_input_tokens",
            "/cache_write_input_tokens",
        ],
    );
    let output = adam_usage_i64(
        usage,
        &[
            "/output_tokens",
            "/completion_tokens",
            "/usage/output_tokens",
            "/usage/completion_tokens",
        ],
    );
    Some(InhouseProviderCacheEvidence {
        backend: "openai".to_string(),
        wire_family: wire_family.to_string(),
        dialect: dialect.to_string(),
        evidence_source: "provider_usage_telemetry".to_string(),
        input_uncached_tokens: input.saturating_sub(cache_read).saturating_sub(cache_write),
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        output_tokens: Some(output),
        cache_hit_rate: None,
        cache_hit: cache_read > 0,
        required_cache_satisfied: Some(cache_read > 0),
        provider_slot_id: None,
        details: Some(usage.clone()),
    })
}

#[async_trait]
pub trait AdamProviderConnector: Send + Sync {
    /// Optionally project a no-tool shared prompt before engine allocation so
    /// cache identity and provider inference consume the same exact bytes.
    async fn project_no_tool_wire_prompt(
        &self,
        _stable_projection: &str,
        _full_projection: &str,
    ) -> Result<Option<LlamaWireProjection>, AdamBackendError> {
        Ok(None)
    }

    async fn run_turn(
        &self,
        request: AdamProviderRequest,
        sink: &mut dyn AdamProviderEventSink,
    ) -> Result<AdamProviderTurnEnd, AdamBackendError>;
}

#[async_trait]
pub trait AdamOAuthCredentialResolver: Send + Sync {
    async fn resolve_adam_oauth_credential(
        &self,
        config: &OpenAiConfig,
    ) -> Result<AdamOAuthCredential, AdamBackendError>;
}

pub struct AdamAgentRunner {
    config: AdamAgentConfig,
    causal_identity_mode: AdamCausalIdentityMode,
    connector_override: Option<Arc<dyn AdamProviderConnector>>,
    reusable_connector: std::sync::OnceLock<Arc<dyn AdamProviderConnector>>,
    oauth_credential_resolver: Option<Arc<dyn AdamOAuthCredentialResolver>>,
    projection_store: Option<Arc<dyn AdamRuntimeProjectionStore>>,
    engine_manager: Option<Arc<AdamInferenceEngineManager>>,
    engine_managed_endpoint_id: String,
    engine_tokenizer_hash: String,
    engine_context_params_hash: String,
}

struct AdamEngineReservationGuard {
    manager: Arc<AdamInferenceEngineManager>,
    allocation: AdamEngineAllocation,
    heartbeat: tokio::task::JoinHandle<()>,
    release_on_drop: bool,
}

impl AdamEngineReservationGuard {
    fn new(manager: Arc<AdamInferenceEngineManager>, allocation: AdamEngineAllocation) -> Self {
        let heartbeat_manager = Arc::clone(&manager);
        let heartbeat_allocation = allocation.clone();
        let heartbeat = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
            interval.tick().await;
            loop {
                interval.tick().await;
                if !heartbeat_manager.heartbeat_allocation(&heartbeat_allocation) {
                    break;
                }
            }
        });
        Self {
            manager,
            allocation,
            heartbeat,
            release_on_drop: true,
        }
    }

    /// Leaves the lease and reservation fenced until their durable TTLs
    /// expire. This is required when cancelling the provider future did not
    /// make the physical slot observably idle within the bounded drain window.
    fn quarantine_until_expiry(&mut self) {
        self.release_on_drop = false;
    }
}

impl Drop for AdamEngineReservationGuard {
    fn drop(&mut self) {
        self.heartbeat.abort();
        if self.release_on_drop {
            self.manager.release_allocation(&self.allocation);
        }
    }
}

struct AdamInterruptedAttemptRecord<'a> {
    attempt_id: &'a str,
    session_id: &'a str,
    request_message_id: &'a str,
    render_projection_id: &'a str,
    provider_attempt_ref: Option<String>,
    provider_response_id: Option<String>,
    attempt_started_at: i64,
    summary: &'a str,
}

async fn record_interrupted_attempt(
    projection_store: Option<&Arc<dyn AdamRuntimeProjectionStore>>,
    record: AdamInterruptedAttemptRecord<'_>,
) -> anyhow::Result<()> {
    if let Some(store) = projection_store {
        store
            .record_turn_attempt(AdamTurnAttempt {
                attempt_id: record.attempt_id.to_string(),
                session_id: record.session_id.to_string(),
                request_message_id: record.request_message_id.to_string(),
                render_projection_id: Some(record.render_projection_id.to_string()),
                status: AdamTurnAttemptStatus::Interrupted,
                provider_attempt_ref: record.provider_attempt_ref,
                provider_response_id: record.provider_response_id,
                error_kind: Some("interrupted".to_string()),
                error_summary: Some(record.summary.to_string()),
                busy_kind: None,
                started_at_unix_ms: record.attempt_started_at,
                ended_at_unix_ms: Some(crate::now_unix_ms()),
            })
            .await?;
    }
    Ok(())
}

async fn settle_interrupted_engine_allocation(
    engine_manager: Option<&Arc<AdamInferenceEngineManager>>,
    engine_allocation: Option<&AdamEngineAllocation>,
    reservation_guard: Option<&mut AdamEngineReservationGuard>,
) {
    let (Some(engine_manager), Some(engine_allocation), Some(reservation_guard)) =
        (engine_manager, engine_allocation, reservation_guard)
    else {
        return;
    };
    if !engine_manager
        .wait_until_allocation_idle(engine_allocation, std::time::Duration::from_secs(5))
        .await
    {
        reservation_guard.quarantine_until_expiry();
    }
}

async fn settle_failed_tool_engine_allocation(
    engine_manager: Option<&Arc<AdamInferenceEngineManager>>,
    engine_allocation: Option<&AdamEngineAllocation>,
    reservation_guard: Option<&mut AdamEngineReservationGuard>,
) {
    let (Some(engine_manager), Some(engine_allocation), Some(reservation_guard)) =
        (engine_manager, engine_allocation, reservation_guard)
    else {
        return;
    };
    if engine_manager.erase_slot(engine_allocation).await.is_err() {
        engine_manager.invalidate_allocation_residency_if_current(engine_allocation);
        reservation_guard.quarantine_until_expiry();
    }
}

async fn persist_tool_terminal(
    projection_store: Option<&Arc<dyn AdamRuntimeProjectionStore>>,
    intent: Option<&AdamToolInvocationEvidence>,
    status: AdamToolInvocationStatus,
    reused_from_invocation_id: Option<String>,
    polar_tool_call_ref: Option<String>,
    polar_tool_result_ref: Option<String>,
    error_summary: Option<String>,
) -> anyhow::Result<()> {
    let (Some(store), Some(intent)) = (projection_store, intent) else {
        return Ok(());
    };
    store
        .record_tool_invocation_evidence(intent.settle(
            status,
            reused_from_invocation_id,
            polar_tool_call_ref,
            polar_tool_result_ref,
            error_summary,
            crate::now_unix_ms(),
        ))
        .await
}

impl AdamAgentRunner {
    pub fn new(config: AdamAgentConfig) -> Self {
        codex_utils_rustls_provider::ensure_rustls_crypto_provider();
        Self {
            config,
            causal_identity_mode: AdamCausalIdentityMode::CausalV2,
            connector_override: None,
            reusable_connector: std::sync::OnceLock::new(),
            oauth_credential_resolver: None,
            projection_store: None,
            engine_manager: None,
            engine_managed_endpoint_id: String::new(),
            engine_tokenizer_hash: String::new(),
            engine_context_params_hash: String::new(),
        }
    }

    pub fn with_connector(
        config: AdamAgentConfig,
        connector: Arc<dyn AdamProviderConnector>,
    ) -> Self {
        codex_utils_rustls_provider::ensure_rustls_crypto_provider();
        Self {
            config,
            causal_identity_mode: AdamCausalIdentityMode::CausalV2,
            connector_override: Some(connector),
            reusable_connector: std::sync::OnceLock::new(),
            oauth_credential_resolver: None,
            projection_store: None,
            engine_manager: None,
            engine_managed_endpoint_id: String::new(),
            engine_tokenizer_hash: String::new(),
            engine_context_params_hash: String::new(),
        }
    }

    pub fn with_oauth_credential_resolver(
        mut self,
        resolver: Arc<dyn AdamOAuthCredentialResolver>,
    ) -> Self {
        self.oauth_credential_resolver = Some(resolver);
        self.reusable_connector = std::sync::OnceLock::new();
        self
    }

    pub fn with_projection_store(mut self, store: Arc<dyn AdamRuntimeProjectionStore>) -> Self {
        self.projection_store = Some(store);
        self
    }

    pub fn with_causal_identity_mode(mut self, mode: AdamCausalIdentityMode) -> Self {
        self.causal_identity_mode = mode;
        self
    }

    pub fn with_engine_manager(mut self, manager: Arc<AdamInferenceEngineManager>) -> Self {
        self.engine_manager = Some(manager);
        self
    }

    pub fn with_engine_compatibility(
        mut self,
        tokenizer_hash: impl Into<String>,
        context_params_hash: impl Into<String>,
    ) -> Self {
        self.engine_tokenizer_hash = tokenizer_hash.into();
        self.engine_context_params_hash = context_params_hash.into();
        self
    }

    pub fn with_managed_endpoint_binding(mut self, endpoint_id: impl Into<String>) -> Self {
        self.engine_managed_endpoint_id = endpoint_id.into();
        self
    }

    /// Returns the opaque runtime-owned endpoint binding used for engine
    /// allocation diagnostics. Slot policy remains inside the Adam runtime.
    pub fn managed_endpoint_binding(&self) -> Option<&str> {
        (!self.engine_managed_endpoint_id.is_empty())
            .then_some(self.engine_managed_endpoint_id.as_str())
    }

    fn connector(&self) -> Result<Arc<dyn AdamProviderConnector>, AdamBackendError> {
        if let Some(connector) = &self.connector_override {
            return Ok(Arc::clone(connector));
        }
        match &self.config.backend.connection {
            AdamConnectionConfig::Llama(local) => {
                let local = local.clone();
                match local.transport {
                    LlamaTransport::LlamacppSlots => Ok(Arc::new(LlamaCppSlotsClient::new(local)?)),
                    LlamaTransport::OpenAiCompatibleResponses => {
                        Ok(Arc::new(OpenAiCompatibleResponsesConnector::new(local)?))
                    }
                    LlamaTransport::OpenAiCompatibleChat => {
                        Ok(Arc::new(OpenAiCompatibleChatConnector::new(local)?))
                    }
                }
            }
            AdamConnectionConfig::OpenAi(config) => {
                let resolver = self.oauth_credential_resolver.clone().ok_or_else(|| {
                    AdamBackendError::Auth(
                        "Adam OpenAI runner requires a Polar vault credential resolver".to_string(),
                    )
                })?;
                Ok(Arc::clone(self.reusable_connector.get_or_init(|| {
                    Arc::new(OpenAiConnector::new(config.clone(), resolver))
                })))
            }
        }
    }
}

fn canonical_responses_input(messages: &[InhouseCanonicalThreadMessage]) -> Value {
    json!(messages.iter().map(|message| json!({"role": message.role, "content": message.text})).collect::<Vec<_>>())
}

type OpenAiSocket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct OpenAiConversationCheckpoint {
    session_id: String,
    messages: Vec<InhouseCanonicalThreadMessage>,
    continuity: crate::openai::OpenAiContinuitySnapshot,
}
struct ReusableOpenAiSocket {
    authorization_key: String,
    socket: OpenAiSocket,
    checkpoint: Option<OpenAiConversationCheckpoint>,
}

struct OpenAiConnector {
    socket: tokio::sync::Mutex<Option<ReusableOpenAiSocket>>,
    config: OpenAiConfig,
    credential_resolver: Arc<dyn AdamOAuthCredentialResolver>,
}

impl OpenAiConnector {
    fn new(
        config: OpenAiConfig,
        credential_resolver: Arc<dyn AdamOAuthCredentialResolver>,
    ) -> Self {
        Self {
            config,
            credential_resolver,
            socket: tokio::sync::Mutex::new(None),
        }
    }
}

#[async_trait]
impl AdamProviderConnector for OpenAiConnector {
    async fn run_turn(
        &self,
        request: AdamProviderRequest,
        sink: &mut dyn AdamProviderEventSink,
    ) -> Result<AdamProviderTurnEnd, AdamBackendError> {
        let credential = self
            .credential_resolver
            .resolve_adam_oauth_credential(&self.config)
            .await?;
        let timeout = Duration::from_secs(self.config.websocket_timeout_secs);
        let websocket_url = openai_responses_websocket_url(&self.config.openai_base_url)
            .map_err(|err| AdamBackendError::Transport(err.to_string()))?;
        let client_request_id = format!(
            "{}:{}",
            request.turn_context.thread_id, request.turn_context.request_message_id
        );
        let headers = build_openai_headers(
            &self.config,
            &credential,
            &request.turn_context,
            &client_request_id,
        );
        let mut websocket_request = websocket_url
            .into_client_request()
            .map_err(|err| AdamBackendError::Transport(err.to_string()))?;
        for (name, value) in headers {
            websocket_request.headers_mut().insert(
                HeaderName::from_bytes(name.as_bytes())
                    .map_err(|err| AdamBackendError::Transport(err.to_string()))?,
                HeaderValue::from_str(&value)
                    .map_err(|err| AdamBackendError::Transport(err.to_string()))?,
            );
        }

        // Take the socket out while a turn is running. Cancellation or any error
        // drops it, so a later request cannot read a cancelled response's events.
        let authorization_key = format!("{:x}", Sha256::digest(format!("{}:{}:{:?}",
            credential.authorization_header(), request.turn_context.owner_id, credential.account_id)));
        let mut reusable = self.socket.lock().await;
        let (mut socket, checkpoint) = match reusable.take().filter(|entry| entry.authorization_key == authorization_key && request.tools.is_empty()) {
            Some(entry) => (entry.socket, entry.checkpoint),
            None => (tokio::time::timeout(timeout, tokio_tungstenite::connect_async(websocket_request))
                .await
                .map_err(|_| AdamBackendError::Transport("Adam OpenAI WebSocket connect timed out".into()))?
                .map_err(openai_websocket_connect_error)?.0, None),
        };
        let mut body = build_openai_response_create_message(
            &self.config,
            &request.turn_context,
            &request.instructions,
            &request.input,
            request.tools.clone(),
            request.responses_items.clone(),
        );
        let prefix_hash = stable_hash_json(&json!({"instructions": request.instructions, "model": self.config.model,
            "reasoning": self.config.reasoning_effort, "parallel_tools": self.config.parallel_tool_calls}));
        let tools_hash = stable_hash_json(&json!(request.tools));
        let full_input = request.canonical_messages.as_ref().map(|messages| canonical_responses_input(messages));
        let mut continued = false;
        if let Some(full_input) = &full_input {
            body["input"] = full_input.clone();
            if let (Some(checkpoint), Some(messages)) = (&checkpoint, &request.canonical_messages) {
                if checkpoint.session_id == request.session_id && messages.len() > checkpoint.messages.len()
                    && messages[..checkpoint.messages.len()] == checkpoint.messages {
                    let input_hash = stable_hash_json(&json!(&messages[..checkpoint.messages.len()]));
                    let decision = crate::openai::decide_openai_previous_response_id(Some(&checkpoint.continuity),
                        &crate::openai::OpenAiContinuityRequest {
                            context_compaction_epoch: 0, source_digest: &input_hash,
                            renderer_version: "canonical-responses-v1", model: &self.config.model,
                            tool_schema_hash: &tools_hash, exact_prefix_hash: &prefix_hash, input_hash: &input_hash,
                        });
                    if let crate::openai::OpenAiPreviousResponseDecision::Reuse { previous_response_id } = decision {
                        body["previous_response_id"] = json!(previous_response_id);
                        body["input"] = canonical_responses_input(&messages[checkpoint.messages.len()..]);
                        continued = true;
                    }
                }
            }
        }
        socket
            .send(Message::Text(body.to_string().into()))
            .await
            .map_err(|err| AdamBackendError::Transport(err.to_string()))?;

        let mut emitted_answer_delta = false;
        let mut response_text = String::new();
        let mut output_items = Vec::new();
        loop {
            let message = tokio::time::timeout(timeout, socket.next())
                .await
                .map_err(|_| {
                    AdamBackendError::Transport(
                        "Adam OpenAI WebSocket event read timed out".to_string(),
                    )
                })?
                .ok_or_else(|| {
                    AdamBackendError::Transport(
                        "Adam OpenAI WebSocket closed before response.completed".to_string(),
                    )
                })?
                .map_err(|err| AdamBackendError::Transport(err.to_string()))?;
            let Some(value) = websocket_message_json(message)? else {
                continue;
            };
            match parse_openai_event(&value) {
                OpenAiResponsesEvent::TextDelta { text } => {
                    if !text.is_empty() {
                        emitted_answer_delta = true;
                        response_text.push_str(&text);
                        sink.emit(AdamProviderEvent::AnswerDelta(text)).await?;
                    }
                }
                OpenAiResponsesEvent::ThinkingDelta { text, source } => {
                    if !text.is_empty() {
                        sink.emit(AdamProviderEvent::ThinkingDelta {
                            text,
                            source,
                            closed: None,
                        })
                        .await?;
                    }
                }
                OpenAiResponsesEvent::OutputItemDone { item } => {
                    if let Some(text) = openai_reasoning_text_from_output_item(&item) {
                        sink.emit(AdamProviderEvent::ThinkingDelta {
                            text,
                            source: AdamThinkingSource::OpenAiReasoningOutputItem,
                            closed: Some(true),
                        })
                        .await?;
                    }
                    sink.emit(AdamProviderEvent::RawOutputItem(item.clone()))
                        .await?;
                    output_items.push(item);
                }
                OpenAiResponsesEvent::Completed {
                    response_id,
                    usage,
                    text,
                    function_calls,
                } => {
                    if !emitted_answer_delta {
                        if let Some(text) = text.filter(|value| !value.is_empty()) {
                            response_text.push_str(&text);
                            sink.emit(AdamProviderEvent::AnswerDelta(text)).await?;
                        }
                    }
                    for call in function_calls {
                        sink.emit(AdamProviderEvent::ToolCall(call)).await?;
                    }
                    let cache_evidence = openai_cache_evidence_from_usage(
                        usage.as_ref(),
                        "openai_responses_ws",
                        "openai_responses_ws",
                    );
                    let checkpoint = request.canonical_messages.clone().zip(response_id.clone()).map(|(mut messages, previous_response_id)| {
                        messages.push(InhouseCanonicalThreadMessage {
                            message_id: format!("{}:result", request.turn_context.request_message_id),
                            role: "assistant".into(), text: response_text.clone(),
                        });
                        let input_hash = stable_hash_json(&json!(&messages));
                        OpenAiConversationCheckpoint { session_id: request.session_id.clone(), messages,
                            continuity: crate::openai::OpenAiContinuitySnapshot {
                                previous_response_id, context_compaction_epoch: 0, source_digest: input_hash.clone(),
                                renderer_version: "canonical-responses-v1".into(), model: self.config.model.clone(),
                                tool_schema_hash: tools_hash, exact_prefix_hash: prefix_hash, input_hash,
                            }
                        }
                    });
                    // This optimization is for tool-free editing. Preserve the
                    // existing tool-loop transport lifecycle and replay contract.
                    if request.tools.is_empty() {
                        *reusable = Some(ReusableOpenAiSocket { authorization_key, socket, checkpoint });
                    }
                    return Ok(AdamProviderTurnEnd {
                        provider_response_id: response_id,
                        usage,
                        cache_evidence,
                        raw: Some(json!({ "output_items": value.get("response")
                            .unwrap_or(&value).get("output")
                            .and_then(Value::as_array).unwrap_or(&output_items) })),
                    });
                }
                OpenAiResponsesEvent::Failed { message } => {
                    if continued && !emitted_answer_delta && (message.contains("previous_response_not_found")
                        || value.pointer("/error/code").and_then(Value::as_str) == Some("previous_response_not_found")) {
                        // The provider explicitly rejected the missing parent before
                        // generation. Re-render once; never replay an ambiguous turn.
                        body.as_object_mut().expect("response object").remove("previous_response_id");
                        body["input"] = full_input.clone().expect("canonical continuation");
                        continued = false;
                        socket.send(Message::Text(body.to_string().into())).await
                            .map_err(|err| AdamBackendError::Transport(err.to_string()))?;
                        continue;
                    }
                    return if openai_message_is_busy(&message) {
                        Err(AdamBackendError::ProviderBusy {
                            kind: AdamBusyKind::OpenAiProviderBusy,
                            message,
                        })
                    } else {
                        Err(AdamBackendError::Provider {
                            status: None,
                            message,
                        })
                    };
                }
                OpenAiResponsesEvent::Ignored => {}
            }
        }
    }
}

fn openai_websocket_connect_error(
    error: tokio_tungstenite::tungstenite::Error,
) -> AdamBackendError {
    if let tokio_tungstenite::tungstenite::Error::Http(response) = &error {
        let status = response.status().as_u16();
        // Do not include response headers or credential material in errors.
        return match status {
            401 => AdamBackendError::Auth("OpenAI rejected the selected OAuth credential (HTTP 401)".into()),
            429 => AdamBackendError::ProviderBusy {
                kind: AdamBusyKind::OpenAiProviderBusy,
                message: "OpenAI rate limited the selected account (HTTP 429)".into(),
            },
            _ => AdamBackendError::Provider {
                status: Some(status),
                message: "OpenAI rejected the WebSocket handshake for the configured account and endpoint".into(),
            },
        };
    }
    AdamBackendError::Transport(error.to_string())
}

fn openai_reasoning_text_from_output_item(item: &Value) -> Option<String> {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
    let is_reasoning = item_type.contains("reasoning");
    if !is_reasoning {
        return None;
    }
    let text = item
        .get("summary")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| {
            part.get("text")
                .or_else(|| part.get("content"))
                .and_then(Value::as_str)
        })
        .chain(
            item.get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|part| {
                    part.get("text")
                        .or_else(|| part.get("content"))
                        .and_then(Value::as_str)
                }),
        )
        .collect::<String>();
    (!text.trim().is_empty()).then_some(text)
}

fn adam_event_handler_error(error: anyhow::Error) -> AdamBackendError {
    AdamBackendError::Transport(format!("Adam turn event handler failed: {error:#}"))
}

pub(crate) fn with_adam_event_sink<T>(
    event_sink: &AdamTurnEventSink,
    action: impl FnOnce(&mut (dyn InhouseTurnEventHandler + Send)) -> anyhow::Result<T>,
) -> Result<T, AdamBackendError> {
    let mut handler = event_sink.lock().map_err(|_| {
        AdamBackendError::Transport("Adam turn event handler lock poisoned".to_string())
    })?;
    action(handler.as_mut()).map_err(adam_event_handler_error)
}

fn websocket_message_json(message: Message) -> Result<Option<Value>, AdamBackendError> {
    match message {
        Message::Text(text) => serde_json::from_str::<Value>(&text)
            .map(Some)
            .map_err(|err| AdamBackendError::Parse(err.to_string())),
        Message::Binary(bytes) => {
            let text = String::from_utf8(bytes.to_vec())
                .map_err(|err| AdamBackendError::Parse(err.to_string()))?;
            serde_json::from_str::<Value>(&text)
                .map(Some)
                .map_err(|err| AdamBackendError::Parse(err.to_string()))
        }
        Message::Close(_) => Err(AdamBackendError::Transport(
            "Adam OpenAI WebSocket closed by provider".to_string(),
        )),
        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => Ok(None),
    }
}

fn openai_tool_outputs(results: &[AdamToolResult]) -> Vec<Value> {
    results
        .iter()
        .map(|result| {
            json!({
                "type": "function_call_output",
                "call_id": result.call_id,
                "output": result.output_text,
            })
        })
        .collect()
}

fn openai_message_is_busy(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    [
        "rate limit",
        "ratelimit",
        "too many requests",
        "busy",
        "overloaded",
        "capacity",
        "try again",
        "temporarily unavailable",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

#[async_trait]
impl InhouseAgentRunner for AdamAgentRunner {
    async fn inspect_session(
        &mut self,
        session_id: &str,
        _include_turns: bool,
    ) -> anyhow::Result<Option<serde_json::Value>> {
        let Some(store) = self.projection_store.as_ref() else {
            return Ok(None);
        };
        Ok(Some(store.inspect_session_projection(session_id).await?))
    }

    async fn preflight_context_budget(
        &mut self,
        input: &str,
        turn_options: &InhouseTurnOptions,
        configured_context_tokens: i64,
        output_reserve_tokens: i64,
    ) -> anyhow::Result<Option<polar_inhouse_contract::InhouseBackendContextBudget>> {
        let Some(engine_manager) = self.engine_manager.as_ref() else {
            return Ok(None);
        };
        let Some(llama) = self
            .config
            .backend
            .llama()
            .filter(|llama| matches!(llama.transport, LlamaTransport::LlamacppSlots))
        else {
            return Ok(None);
        };
        let request = turn_options.backend_turn_request.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Adam context preflight requires an assembled backend turn request")
        })?;
        let context = AdamTurnContext::from_request(request);
        let prepared = prepare_adam_prompt_input(input, turn_options);
        let provider_input = prepared.provider_input(self.config.backend.connection_mode());
        let connector = self.connector().map_err(AdamBackendError::into_anyhow)?;
        let wire = connector
            .project_no_tool_wire_prompt(
                prepared.stable_prefix.as_deref().unwrap_or_default(),
                &provider_input,
            )
            .await
            .map_err(AdamBackendError::into_anyhow)?
            .ok_or_else(|| {
                anyhow::anyhow!("Adam managed llama exact wire projection unavailable")
            })?;
        let (probe, exact_prompt_tokens) = engine_manager
            .count_wire_prompt_tokens(&self.engine_managed_endpoint_id, &wire.full_prompt)
            .await?;
        let budget = polar_inhouse_contract::InhouseBackendContextBudget::from_verified_live_count(
            probe.max_context_tokens,
            configured_context_tokens,
            output_reserve_tokens.max(llama.max_output_tokens),
            512,
            exact_prompt_tokens,
        )
        .map_err(anyhow::Error::msg)?;
        tracing::info!(
            thread_id = %context.thread_id,
            max_context_tokens = budget.max_context_tokens,
            exact_prompt_tokens = budget.exact_prompt_tokens,
            effective_input_budget_tokens = budget.effective_input_budget_tokens,
            "Adam exact live context admission preflight"
        );
        Ok(Some(budget))
    }

    async fn run_turn(
        &mut self,
        existing_session_id: Option<String>,
        input: String,
        turn_options: InhouseTurnOptions,
        dynamic_tool_handler: Option<Arc<dyn InhouseDynamicToolHandler>>,
    ) -> anyhow::Result<(String, String)> {
        self.run_turn_streaming(
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
        match self
            .run_turn_streaming_outcome(
                existing_session_id,
                input,
                turn_options,
                event_handler,
                turn_control,
                dynamic_tool_handler,
            )
            .await?
        {
            InhouseAgentTurnOutcome::Immediate {
                session_id,
                reply_text,
            } => Ok((session_id, reply_text)),
            InhouseAgentTurnOutcome::ImmediateOutput(output) => {
                Ok((output.session_id, output.reply_text))
            }
            InhouseAgentTurnOutcome::Deferred {
                session_id,
                status_text,
                ..
            } => Ok((session_id, status_text)),
        }
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
        ensure_adam_capability_negotiation(&turn_options)?;
        let request = turn_options.backend_turn_request.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "Adam backend requires an assembled InhouseBackendTurnRequest; legacy options must be adapted above the runner boundary"
            )
        })?;
        let context = AdamTurnContext::from_request(request);
        let binding_key = context.binding_key();
        let projection_store = self.projection_store.clone();
        // The shared runtime alone decides resume vs new. None is a fresh generation,
        // deterministically keyed by the persisted initiating request for retries.
        let session_id = existing_session_id.unwrap_or_else(|| context.stable_session_id());
        if let Some(store) = &projection_store {
            store
                .upsert_session_binding(AdamSessionBinding::new(
                    binding_key,
                    session_id.clone(),
                    self.config.backend.connection_mode().as_str(),
                    adam_transport_label(&self.config),
                ))
                .await?;
        }
        let connector = self.connector().map_err(AdamBackendError::into_anyhow)?;
        let tool_plan = adam_tool_plan(&context, &turn_options, dynamic_tool_handler.as_ref());
        let tools = adam_provider_tool_schemas(&tool_plan);
        let prepared = prepare_adam_prompt_input(&input, &turn_options);
        let provider_input = prepared.provider_input(self.config.backend.connection_mode());
        let wire_projection = if tools.is_empty() {
            connector
                .project_no_tool_wire_prompt(
                    prepared.stable_prefix.as_deref().unwrap_or_default(),
                    &provider_input,
                )
                .await
                .map_err(AdamBackendError::into_anyhow)?
        } else {
            None
        };
        let context_compaction_epoch = adam_context_compaction_epoch(&turn_options);
        let source_refs = adam_projection_source_refs(&context, &turn_options);
        let prompt_source_ref_count = source_refs.len();
        let projection_hashes = compute_adam_runner_projection_hashes(&AdamRunnerHashInput {
            source_refs: source_refs.clone(),
            renderer_version: ADAM_RENDERER_VERSION.to_string(),
            template: prepared.instructions.clone(),
            exact_prefix: prepared.stable_prefix.clone().unwrap_or_default(),
            input: provider_input.clone(),
            tools: tools.clone(),
        });
        let engine_exact_prefix_hash = wire_projection
            .as_ref()
            .map(|wire| crate::llama::llamacpp_wire_prompt_hash(&wire.exact_prefix))
            .unwrap_or_else(|| projection_hashes.exact_prefix_hash.clone());
        let render_projection_id =
            adam_render_projection_id(&session_id, &context.request_message_id, &projection_hashes);
        if let Some(store) = &projection_store {
            if context_compaction_epoch > 0 {
                let invalidation = invalidate_adam_context_compaction_epoch(
                    store.as_ref(),
                    &session_id,
                    context_compaction_epoch,
                )
                .await?;
                if invalidation
                    .previous_context_compaction_epoch
                    .is_some_and(|previous| previous != invalidation.new_context_compaction_epoch)
                {
                    if let Some(engine_manager) = self.engine_manager.as_ref() {
                        engine_manager.invalidate_prepared_compact_slots_for_context_epoch(
                            &session_id,
                            context_compaction_epoch,
                        );
                    }
                }
            }
            store
                .put_render_projection(AdamRenderProjection {
                    projection_id: render_projection_id.clone(),
                    session_id: session_id.clone(),
                    canonical_source_refs: source_refs,
                    source_digest: projection_hashes.source_digest.clone(),
                    context_compaction_epoch,
                    prompt_asset_revision: "adam-prompt-assets-v1".to_string(),
                    renderer_version: projection_hashes.renderer_version.clone(),
                    model_id: self.config.backend.model.clone(),
                    template_hash: projection_hashes.template_hash.clone(),
                    tool_schema_hash: projection_hashes.tool_schema_hash.clone(),
                    exact_prefix_hash: projection_hashes.exact_prefix_hash.clone(),
                    input_hash: projection_hashes.input_hash.clone(),
                    token_count: None,
                    valid: true,
                    invalidation_reason: None,
                    created_at_unix_ms: crate::now_unix_ms(),
                })
                .await?;
        }
        let native_llama = self
            .config
            .backend
            .llama()
            .filter(|llama| matches!(llama.transport, LlamaTransport::LlamacppSlots));
        let scheduling_priority = adam_engine_turn_priority(&turn_options);
        if let Some(engine_manager) = self.engine_manager.as_ref() {
            engine_manager
                .discard_nonmatching_promoted_prefixes(&session_id, &engine_exact_prefix_hash);
        }
        let engine_allocation = match (self.engine_manager.as_ref(), native_llama) {
            (Some(engine_manager), Some(llama)) => Some(
                engine_manager
                    .allocate_with_wait(
                        AdamEngineControlIntent {
                            managed_endpoint_id: self.engine_managed_endpoint_id.clone(),
                            inhouse_polar_id: context.inhouse_polar_id.clone(),
                            thread_id: context.thread_id.clone(),
                            projection_id: render_projection_id.clone(),
                            exact_prefix_hash: engine_exact_prefix_hash.clone(),
                            model_id: self.config.backend.model.clone(),
                            tokenizer_hash: self.engine_tokenizer_hash.clone(),
                            connector_base_url: normalize_connector_url(&llama.base_url),
                            transport: llama.transport.as_str().to_string(),
                            context_params_hash: self.engine_context_params_hash.clone(),
                            context_compaction_epoch,
                            priority: scheduling_priority,
                        },
                        AdamEngineAllocationWaitConfig::default(),
                        turn_control.clone(),
                    )
                    .await?,
            ),
            _ => None,
        };
        let mut engine_reservation_guard = engine_allocation.as_ref().and_then(|allocation| {
            self.engine_manager.as_ref().map(|manager| {
                AdamEngineReservationGuard::new(Arc::clone(manager), allocation.clone())
            })
        });
        let mut tool_results = Vec::new();
        let mut responses_items = Vec::new();
        let mut executed_tool_results: HashMap<String, (AdamToolResult, Option<String>)> =
            HashMap::new();
        let mut observed_tool_call_ids = Vec::new();
        let event_sink = event_handler.map(|handler| Arc::new(Mutex::new(handler)));

        for iteration in 0..=self.config.backend.max_tool_iterations {
            if turn_control
                .as_ref()
                .is_some_and(|control| control.stop_requested())
            {
                anyhow::bail!("Adam turn interrupted before provider request");
            }
            let attempt_id = adam_turn_attempt_id(&context, iteration);
            let attempt_started_at = crate::now_unix_ms();
            if let Some(store) = &projection_store {
                store
                    .record_turn_attempt(AdamTurnAttempt {
                        attempt_id: attempt_id.clone(),
                        session_id: session_id.clone(),
                        request_message_id: context.request_message_id.clone(),
                        render_projection_id: Some(render_projection_id.clone()),
                        status: AdamTurnAttemptStatus::Started,
                        provider_attempt_ref: None,
                        provider_response_id: None,
                        error_kind: None,
                        error_summary: None,
                        busy_kind: None,
                        started_at_unix_ms: attempt_started_at,
                        ended_at_unix_ms: None,
                    })
                    .await?;
            }
            let mut accumulator = AdamTurnAccumulator::with_protected_instructions(
                event_sink.clone(),
                &prepared.instructions,
            );
            let provider_request = AdamProviderRequest {
                canonical_messages: (turn_options.conversation_encoding == polar_inhouse_contract::InhouseConversationEncoding::CanonicalMessages
                    && tools.is_empty()).then(|| turn_options.canonical_thread_messages.clone()),
                session_id: session_id.clone(),
                turn_context: context.clone(),
                instructions: prepared.instructions.clone(),
                stable_prefix: prepared.stable_prefix.clone(),
                input: provider_input.clone(),
                tools: tools.clone(),
                tool_results: tool_results.clone(),
                responses_items: responses_items.clone(),
                prompt_cache_key: Some(prompt_cache_key(&context)),
                provider_slot_id: None,
                max_output_tokens: None,
                wire_projection: wire_projection.clone(),
            };
            let provider_request = match engine_allocation.as_ref() {
                Some(engine_allocation) => AdamProviderRequest {
                    provider_slot_id: Some(engine_allocation.lease.slot_id),
                    ..provider_request
                },
                None => provider_request,
            };
            let provider_request_evidence = serde_json::to_value(&provider_request)?;
            let provider_attempt_ref = turn_options
                .provider_attempt_gate
                .prepare("adam", "turn.dispatch", &provider_request_evidence)
                .await
                .context("persist Adam provider attempt before connector contact")?;
            if let Some(store) = &projection_store {
                store
                    .record_turn_attempt(AdamTurnAttempt {
                        attempt_id: attempt_id.clone(),
                        session_id: session_id.clone(),
                        request_message_id: context.request_message_id.clone(),
                        render_projection_id: Some(render_projection_id.clone()),
                        status: AdamTurnAttemptStatus::Started,
                        provider_attempt_ref: provider_attempt_ref.clone(),
                        provider_response_id: None,
                        error_kind: None,
                        error_summary: None,
                        busy_kind: None,
                        started_at_unix_ms: attempt_started_at,
                        ended_at_unix_ms: None,
                    })
                    .await?;
            }
            let provider_result = {
                let provider_future = connector.run_turn(provider_request, &mut accumulator);
                tokio::pin!(provider_future);
                if let Some(control) = turn_control.as_ref() {
                    tokio::select! {
                        result = &mut provider_future => Some(result),
                        _ = control.wait_for_stop() => None,
                    }
                } else {
                    Some(provider_future.await)
                }
            };
            let Some(provider_result) = provider_result else {
                // Settle the physical allocation before the fallible evidence
                // write. Otherwise a projection-store error would drop the
                // guard and recycle a slot whose server-side request may still
                // be running after the client future was cancelled.
                settle_interrupted_engine_allocation(
                    self.engine_manager.as_ref(),
                    engine_allocation.as_ref(),
                    engine_reservation_guard.as_mut(),
                )
                .await;
                record_interrupted_attempt(
                    projection_store.as_ref(),
                    AdamInterruptedAttemptRecord {
                        attempt_id: &attempt_id,
                        session_id: &session_id,
                        request_message_id: &context.request_message_id,
                        render_projection_id: &render_projection_id,
                        provider_attempt_ref: provider_attempt_ref.clone(),
                        provider_response_id: None,
                        attempt_started_at,
                        summary: "Adam turn interrupted while provider request was in flight.",
                    },
                )
                .await?;
                anyhow::bail!("Adam turn interrupted while provider request was in flight");
            };
            let end = match provider_result {
                Ok(end) => end,
                Err(error) => {
                    if error.is_progress_timeout() {
                        settle_interrupted_engine_allocation(
                            self.engine_manager.as_ref(),
                            engine_allocation.as_ref(),
                            engine_reservation_guard.as_mut(),
                        )
                        .await;
                        let status_text = "Adam did not receive provider progress before the deadline. The attempt is incomplete and safe to retry.";
                        record_interrupted_attempt(
                            projection_store.as_ref(),
                            AdamInterruptedAttemptRecord {
                                attempt_id: &attempt_id,
                                session_id: &session_id,
                                request_message_id: &context.request_message_id,
                                render_projection_id: &render_projection_id,
                                provider_attempt_ref: provider_attempt_ref.clone(),
                                provider_response_id: None,
                                attempt_started_at,
                                summary: status_text,
                            },
                        )
                        .await?;
                        return Ok(InhouseAgentTurnOutcome::ImmediateOutput(
                            InhouseAgentTurnOutput::incomplete(
                                session_id,
                                status_text,
                                polar_inhouse_contract::InhouseTurnArtifacts::default(),
                                InhouseIncompleteTurn::provider_timeout(status_text),
                            ),
                        ));
                    }
                    if let Some(store) = &projection_store {
                        let (status, busy_kind) = match &error {
                            AdamBackendError::ProviderBusy { kind, .. } => (
                                AdamTurnAttemptStatus::BackendBusy,
                                Some(kind.as_str().to_string()),
                            ),
                            _ => (AdamTurnAttemptStatus::Failed, None),
                        };
                        store
                            .record_turn_attempt(AdamTurnAttempt {
                                attempt_id: attempt_id.clone(),
                                session_id: session_id.clone(),
                                request_message_id: context.request_message_id.clone(),
                                render_projection_id: Some(render_projection_id.clone()),
                                status,
                                provider_attempt_ref: provider_attempt_ref.clone(),
                                provider_response_id: None,
                                error_kind: Some(
                                    match status {
                                        AdamTurnAttemptStatus::BackendBusy => "provider_busy",
                                        _ => "provider_error",
                                    }
                                    .to_string(),
                                ),
                                error_summary: Some(error.to_string()),
                                busy_kind,
                                started_at_unix_ms: attempt_started_at,
                                ended_at_unix_ms: Some(crate::now_unix_ms()),
                            })
                            .await?;
                    }
                    return Err(error.into_anyhow());
                }
            };
            if turn_control
                .as_ref()
                .is_some_and(|control| control.stop_requested())
            {
                // The provider may complete after cancellation. Do not convert
                // that stale response into LGP-visible output or engine
                // completion evidence. Dropping the reservation guard releases
                // both the persistent fencing lease and the turn reservation.
                settle_interrupted_engine_allocation(
                    self.engine_manager.as_ref(),
                    engine_allocation.as_ref(),
                    engine_reservation_guard.as_mut(),
                )
                .await;
                record_interrupted_attempt(
                    projection_store.as_ref(),
                    AdamInterruptedAttemptRecord {
                        attempt_id: &attempt_id,
                        session_id: &session_id,
                        request_message_id: &context.request_message_id,
                        render_projection_id: &render_projection_id,
                        provider_attempt_ref: provider_attempt_ref.clone(),
                        provider_response_id: end.provider_response_id.clone(),
                        attempt_started_at,
                        summary: "Adam turn interrupted after provider response.",
                    },
                )
                .await?;
                anyhow::bail!("Adam turn interrupted after provider response");
            }
            // Reuse the provider's completed output, including function calls and
            // opaque reasoning, before adding this iteration's tool results.
            if let Some(items) = end
                .raw
                .as_ref()
                .and_then(|raw| raw.get("output_items").or_else(|| raw.get("output")))
                .and_then(Value::as_array)
            {
                responses_items.extend(items.iter().cloned());
            }
            let mut response = accumulator.finish(session_id.clone(), end);
            if let (Some(engine_manager), Some(engine_allocation)) =
                (self.engine_manager.as_ref(), engine_allocation.as_ref())
            {
                ensure_provider_slot_matches_engine_lease(
                    response.cache_evidence.as_ref(),
                    engine_allocation,
                )?;
                let cache_hit_tokens = response
                    .cache_evidence
                    .as_ref()
                    .map(|evidence| evidence.cache_read_tokens);
                let cache_miss_tokens = response.cache_evidence.as_ref().map(|evidence| {
                    evidence
                        .input_uncached_tokens
                        .saturating_add(evidence.cache_write_tokens)
                });
                engine_manager.record_engine_evidence(
                    engine_allocation,
                    cache_hit_tokens,
                    cache_miss_tokens,
                )?;
                attach_engine_control_evidence(&mut response, engine_allocation);
            }
            if let Some(store) = &projection_store {
                store
                    .record_turn_attempt(AdamTurnAttempt {
                        attempt_id: attempt_id.clone(),
                        session_id: session_id.clone(),
                        request_message_id: context.request_message_id.clone(),
                        render_projection_id: Some(render_projection_id.clone()),
                        status: AdamTurnAttemptStatus::Completed,
                        provider_attempt_ref: provider_attempt_ref.clone(),
                        provider_response_id: response.provider_response_id.clone(),
                        error_kind: None,
                        error_summary: None,
                        busy_kind: None,
                        started_at_unix_ms: attempt_started_at,
                        ended_at_unix_ms: Some(crate::now_unix_ms()),
                    })
                    .await?;
                store
                    .record_usage_telemetry(AdamProviderUsageTelemetry {
                        telemetry_id: format!(
                            "adam_usage_{}",
                            stable_hash_json(&json!({ "attempt_id": attempt_id }))
                        ),
                        session_id: session_id.clone(),
                        cache_epoch_id: None,
                        turn_attempt_id: Some(attempt_id.clone()),
                        provider: self.config.backend.connection_mode().as_str().to_string(),
                        model_id: self.config.backend.model.clone(),
                        counters: adam_usage_counters(
                            self.config.backend.connection_mode(),
                            response.usage.as_ref(),
                            response.cache_evidence.as_ref(),
                        ),
                        accounting_source: AdamUsageAccountingSource::ProviderReported,
                        created_at_unix_ms: crate::now_unix_ms(),
                    })
                    .await?;
            }

            if response.tool_calls.is_empty() {
                if let (Some(store), Some(engine_allocation)) =
                    (projection_store.as_ref(), engine_allocation.as_ref())
                {
                    store
                        .record_run_report(crate::status::AdamRunReport::from_completed_turn(
                            attempt_id.clone(),
                            context.request_message_id.clone(),
                            format!("{}-reply", context.request_message_id),
                            engine_allocation,
                            crate::status::AdamRunCompletionFacts {
                                prompt_source_ref_count,
                                context_compaction_epoch,
                                compaction_id: turn_options
                                    .context_compaction
                                    .as_ref()
                                    .map(|record| record.compaction_id.clone()),
                                tool_call_ids: observed_tool_call_ids.clone(),
                                tool_result_count: tool_results.len(),
                                provider_cache_read_tokens: response
                                    .cache_evidence
                                    .as_ref()
                                    .map_or(0, |evidence| evidence.cache_read_tokens),
                                provider_cache_write_tokens: response
                                    .cache_evidence
                                    .as_ref()
                                    .map_or(0, |evidence| evidence.cache_write_tokens),
                                provider_input_uncached_tokens: response
                                    .cache_evidence
                                    .as_ref()
                                    .map_or(0, |evidence| evidence.input_uncached_tokens),
                                provider_cache_hit_rate_basis_points: response
                                    .cache_evidence
                                    .as_ref()
                                    .and_then(|evidence| evidence.cache_hit_rate)
                                    .and_then(cache_hit_rate_basis_points),
                                provider_cache_hit: response
                                    .cache_evidence
                                    .as_ref()
                                    .map(|evidence| evidence.cache_hit),
                                final_status: if response.no_answer_after_thinking {
                                    "incomplete_no_answer_after_thinking".to_string()
                                } else {
                                    "completed".to_string()
                                },
                                tool_error_kind: None,
                                provider_error_kind: None,
                                provider_response_id: response.provider_response_id.clone(),
                            },
                        ))
                        .await?;
                }
                let output = if response.no_answer_after_thinking {
                    InhouseAgentTurnOutput::incomplete(
                        response.session_id,
                        "Adam could not produce a final answer for this turn.",
                        response.artifacts,
                        InhouseIncompleteTurn::new(
                            "adam_no_answer_after_thinking",
                            "Adam produced thinking but no final answer text.",
                        ),
                    )
                } else {
                    InhouseAgentTurnOutput::with_artifacts(
                        response.session_id,
                        response.text,
                        response.artifacts,
                    )
                };
                return Ok(InhouseAgentTurnOutcome::ImmediateOutput(output));
            }
            let mut materialized_calls = Vec::new();
            for (index, call) in std::mem::take(&mut response.tool_calls)
                .into_iter()
                .enumerate()
            {
                let call_ordinal = u32::try_from(index + 1).unwrap_or(u32::MAX);
                let invocation_id = adam_tool_invocation_id(
                    &session_id,
                    &context.request_message_id,
                    &attempt_id,
                    call_ordinal,
                );
                let intent =
                    (self.causal_identity_mode == AdamCausalIdentityMode::CausalV2).then(|| {
                        AdamToolInvocationEvidence {
                            invocation_id,
                            session_id: session_id.clone(),
                            request_message_id: context.request_message_id.clone(),
                            turn_attempt_id: attempt_id.clone(),
                            provider_attempt_ref: provider_attempt_ref.clone(),
                            provider_response_id: response.provider_response_id.clone(),
                            provider_call_id: call.provider_call_id.clone(),
                            wire_call_id: call.call_id.clone(),
                            call_ordinal,
                            tool_name: call.name.clone(),
                            arguments_hash: adam_tool_arguments_hash(&call.arguments),
                            status: AdamToolInvocationStatus::IntentRecorded,
                            reused_from_invocation_id: None,
                            polar_tool_call_ref: None,
                            polar_tool_result_ref: None,
                            error_summary: None,
                            created_at_unix_ms: crate::now_unix_ms(),
                            updated_at_unix_ms: crate::now_unix_ms(),
                        }
                    });
                if let (Some(store), Some(intent)) = (projection_store.as_ref(), intent.as_ref()) {
                    store
                        .record_tool_invocation_evidence(intent.clone())
                        .await?;
                }
                materialized_calls.push((call, intent));
            }
            if iteration >= self.config.backend.max_tool_iterations {
                for (_, intent) in &materialized_calls {
                    persist_tool_terminal(
                        projection_store.as_ref(),
                        intent.as_ref(),
                        AdamToolInvocationStatus::Rejected,
                        None,
                        None,
                        None,
                        Some("tool_loop_limit".to_string()),
                    )
                    .await?;
                }
                settle_failed_tool_engine_allocation(
                    self.engine_manager.as_ref(), engine_allocation.as_ref(), engine_reservation_guard.as_mut(),
                ).await;
                return Err(AdamBackendError::ToolLoopLimit {
                    max_tool_iterations: self.config.backend.max_tool_iterations,
                }
                .into_anyhow());
            }
            let Some(handler) = dynamic_tool_handler.as_ref() else {
                for (_, intent) in &materialized_calls {
                    persist_tool_terminal(
                        projection_store.as_ref(),
                        intent.as_ref(),
                        AdamToolInvocationStatus::Rejected,
                        None,
                        None,
                        None,
                        Some("dynamic_tool_handler_unavailable".to_string()),
                    )
                    .await?;
                }
                anyhow::bail!(
                    "Adam provider requested tools but no dynamic tool handler is available"
                );
            };
            let previous_result_count = tool_results.len();
            for (call, intent) in materialized_calls {
                let normalized = match normalize_adam_provider_tool_call(
                    &tool_plan,
                    call.call_id,
                    call.provider_call_id,
                    call.name,
                    call.arguments,
                ) {
                    Ok(normalized) => normalized,
                    Err(error) => {
                        persist_tool_terminal(
                            projection_store.as_ref(),
                            intent.as_ref(),
                            AdamToolInvocationStatus::Rejected,
                            None,
                            None,
                            None,
                            Some(error.to_string()),
                        )
                        .await?;
                        return Err(error);
                    }
                };
                observed_tool_call_ids.push(normalized.call_id.clone());
                let mut execution_input = json!({
                    "tool_name": normalized.tool_name, "arguments": normalized.arguments,
                });
                execution_input.sort_all_objects();
                let execution_key = stable_hash_json(&execution_input);
                if let Some((previous, source_invocation_id)) =
                    executed_tool_results.get(&execution_key)
                {
                    persist_tool_terminal(
                        projection_store.as_ref(),
                        intent.as_ref(),
                        AdamToolInvocationStatus::Reused,
                        source_invocation_id.clone(),
                        None,
                        Some(format!(
                            "inhouse-tool-result:{}:{}:reused",
                            context.request_message_id, normalized.call_id
                        )),
                        None,
                    )
                    .await?;
                    tool_results.push(AdamToolResult {
                        call_id: normalized.call_id,
                        name: previous.name.clone(),
                        arguments: previous.arguments.clone(),
                        output_text: previous.output_text.clone(),
                    });
                    continue;
                }
                if let Some(sink) = &event_sink {
                    with_adam_event_sink(sink, |handler| {
                        Ok(handler.on_tool_started(&normalized))
                    })
                    .map_err(AdamBackendError::into_anyhow)?.await?;
                }
                let result = execute_adam_tool_call(handler.as_ref(), &normalized).await?;
                let causal_tool_call_ref = intent
                    .as_ref()
                    .map(|intent| format!("inhouse-tool-invocation:{}", intent.invocation_id));
                let causal_tool_result_ref = intent.as_ref().map(|intent| {
                    format!(
                        "inhouse-tool-result:{}:{}",
                        intent.invocation_id,
                        if result.ok { "ok" } else { "error" }
                    )
                });
                persist_tool_terminal(
                    projection_store.as_ref(),
                    intent.as_ref(),
                    if result.ok {
                        AdamToolInvocationStatus::Completed
                    } else {
                        AdamToolInvocationStatus::Failed
                    },
                    None,
                    causal_tool_call_ref,
                    causal_tool_result_ref,
                    result.error_kind.clone(),
                )
                .await?;
                if let Some(store) = &projection_store {
                    let legacy_provider_call_id = normalized
                        .provider_call_id
                        .clone()
                        .unwrap_or_else(|| normalized.call_id.clone());
                    store
                        .record_tool_call_correlation(AdamToolCallCorrelation {
                            correlation_id: format!(
                                "adam_tool_correlation_{}",
                                stable_hash_json(&json!({
                                    "session_id": session_id,
                                    "request_message_id": context.request_message_id,
                                    "provider_call_id": legacy_provider_call_id,
                                }))
                            ),
                            session_id: session_id.clone(),
                            provider_call_id: legacy_provider_call_id,
                            polar_tool_call_ref: Some(format!(
                                "inhouse-tool-call:{}:{}",
                                context.request_message_id, result.call_id
                            )),
                            polar_tool_result_ref: Some(format!(
                                "inhouse-tool-result:{}:{}:{}",
                                context.request_message_id,
                                result.call_id,
                                if result.ok { "ok" } else { "error" }
                            )),
                            status: if result.ok {
                                "completed".to_string()
                            } else {
                                "failed".to_string()
                            },
                            error_summary: result.error_kind.clone(),
                            created_at_unix_ms: crate::now_unix_ms(),
                        })
                        .await?;
                }
                if let Some(sink) = &event_sink {
                    with_adam_event_sink(sink, |handler| {
                        Ok(handler.on_tool_finished(&result))
                    })
                    .map_err(AdamBackendError::into_anyhow)?.await?;
                }
                if !result.ok && !result.argument_correction_allowed {
                    let tool_error_kind = result
                        .error_kind
                        .clone()
                        .unwrap_or_else(|| "execution_failed".to_string());
                    let retry_guidance = result.retry_guidance.clone().unwrap_or_else(|| {
                        "Retry the request. If it fails again, ask an operator to inspect the correlated tool event."
                            .to_string()
                    });
                    settle_failed_tool_engine_allocation(
                        self.engine_manager.as_ref(),
                        engine_allocation.as_ref(),
                        engine_reservation_guard.as_mut(),
                    )
                    .await;
                    if let Some(store) = &projection_store {
                        store
                            .record_turn_attempt(AdamTurnAttempt {
                                attempt_id: attempt_id.clone(),
                                session_id: session_id.clone(),
                                request_message_id: context.request_message_id.clone(),
                                render_projection_id: Some(render_projection_id.clone()),
                                status: AdamTurnAttemptStatus::Failed,
                                provider_attempt_ref: provider_attempt_ref.clone(),
                                provider_response_id: response.provider_response_id.clone(),
                                error_kind: Some("tool_execution_failed".to_string()),
                                error_summary: Some(tool_error_kind.clone()),
                                busy_kind: None,
                                started_at_unix_ms: attempt_started_at,
                                ended_at_unix_ms: Some(crate::now_unix_ms()),
                            })
                            .await?;
                        if let Some(engine_allocation) = engine_allocation.as_ref() {
                            store
                                .record_run_report(
                                    crate::status::AdamRunReport::from_completed_turn(
                                        attempt_id.clone(),
                                        context.request_message_id.clone(),
                                        format!("{}-reply", context.request_message_id),
                                        engine_allocation,
                                        crate::status::AdamRunCompletionFacts {
                                            prompt_source_ref_count,
                                            context_compaction_epoch,
                                            compaction_id: turn_options
                                                .context_compaction
                                                .as_ref()
                                                .map(|record| record.compaction_id.clone()),
                                            tool_call_ids: observed_tool_call_ids.clone(),
                                            tool_result_count: tool_results.len() + 1,
                                            provider_cache_read_tokens: response
                                                .cache_evidence
                                                .as_ref()
                                                .map_or(0, |evidence| evidence.cache_read_tokens),
                                            provider_cache_write_tokens: response
                                                .cache_evidence
                                                .as_ref()
                                                .map_or(0, |evidence| evidence.cache_write_tokens),
                                            provider_input_uncached_tokens: response
                                                .cache_evidence
                                                .as_ref()
                                                .map_or(0, |evidence| {
                                                    evidence.input_uncached_tokens
                                                }),
                                            provider_cache_hit_rate_basis_points: response
                                                .cache_evidence
                                                .as_ref()
                                                .and_then(|evidence| evidence.cache_hit_rate)
                                                .and_then(cache_hit_rate_basis_points),
                                            provider_cache_hit: response
                                                .cache_evidence
                                                .as_ref()
                                                .map(|evidence| evidence.cache_hit),
                                            final_status: "incomplete_tool_failure".to_string(),
                                            tool_error_kind: Some(tool_error_kind.clone()),
                                            provider_error_kind: Some("not_applicable".to_string()),
                                            provider_response_id: response
                                                .provider_response_id
                                                .clone(),
                                        },
                                    ),
                                )
                                .await?;
                        }
                    }
                    let reply_text = format!(
                        "The Polar tool did not complete: {} {}",
                        result.result_text, retry_guidance
                    );
                    return Ok(InhouseAgentTurnOutcome::ImmediateOutput(
                        InhouseAgentTurnOutput::incomplete(
                            response.session_id,
                            reply_text,
                            response.artifacts,
                            InhouseIncompleteTurn::tool_failure(
                                tool_error_kind,
                                result.result_text,
                                retry_guidance,
                            ),
                        ),
                    ));
                }
                let output_text = crate::tool_bridge::model_tool_result(&result);
                let tool_result = AdamToolResult {
                    call_id: result.call_id,
                    name: result.tool_name,
                    arguments: result.arguments,
                    output_text,
                };
                executed_tool_results.insert(
                    execution_key,
                    (
                        tool_result.clone(),
                        intent.as_ref().map(|intent| intent.invocation_id.clone()),
                    ),
                );
                tool_results.push(tool_result);
            }
            responses_items.extend(openai_tool_outputs(&tool_results[previous_result_count..]));
        }

        Err(AdamBackendError::ToolLoopLimit {
            max_tool_iterations: self.config.backend.max_tool_iterations,
        }
        .into_anyhow())
    }
}

fn cache_hit_rate_basis_points(rate: f64) -> Option<u32> {
    rate.is_finite()
        .then(|| (rate.clamp(0.0, 1.0) * 10_000.0).round() as u32)
}

fn ensure_provider_slot_matches_engine_lease(
    cache_evidence: Option<&InhouseProviderCacheEvidence>,
    engine_allocation: &AdamEngineAllocation,
) -> anyhow::Result<()> {
    let Some(provider_slot_id) = cache_evidence.and_then(|evidence| evidence.provider_slot_id)
    else {
        return Ok(());
    };
    if provider_slot_id != engine_allocation.lease.slot_id {
        anyhow::bail!(
            "Adam provider cache evidence slot {provider_slot_id} does not match engine lease slot {}",
            engine_allocation.lease.slot_id
        );
    }
    Ok(())
}

fn attach_engine_control_evidence(
    response: &mut AdamAccumulatedTurn,
    engine_allocation: &AdamEngineAllocation,
) {
    let engine_details = json!({
        "engine_control_evidence": engine_allocation.evidence,
        "cache_plan": engine_allocation.cache_plan.as_str(),
    });
    let evidence = response
        .cache_evidence
        .get_or_insert_with(|| InhouseProviderCacheEvidence {
            backend: "adam".to_string(),
            wire_family: "llama".to_string(),
            dialect: "adam_engine_manager".to_string(),
            evidence_source: "confirmed_provider_completion".to_string(),
            input_uncached_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: None,
            cache_hit_rate: None,
            cache_hit: matches!(
                engine_allocation.cache_plan,
                AdamCachePlan::UseLiveSlot { .. }
            ),
            required_cache_satisfied: None,
            provider_slot_id: Some(engine_allocation.lease.slot_id),
            details: None,
        });
    evidence.provider_slot_id = Some(engine_allocation.lease.slot_id);
    evidence.details = Some(match evidence.details.take() {
        Some(Value::Object(mut details)) => {
            details.insert("adam_engine_control".to_string(), engine_details);
            Value::Object(details)
        }
        Some(provider_details) => json!({
            "provider": provider_details,
            "adam_engine_control": engine_details,
        }),
        None => json!({ "adam_engine_control": engine_details }),
    });
    response.artifacts.cache_evidence = vec![evidence.clone()];
}

#[derive(Debug, Clone)]
struct PreparedAdamInput {
    instructions: String,
    stable_prefix: Option<String>,
    input: String,
    full_prompt: String,
}

impl PreparedAdamInput {
    fn provider_input(&self, mode: AdamLlmConnectionMode) -> String {
        match mode {
            AdamLlmConnectionMode::Llama => self.full_prompt.clone(),
            AdamLlmConnectionMode::OpenAi => self.input.clone(),
        }
    }
}

fn prepare_adam_prompt_input(input: &str, options: &InhouseTurnOptions) -> PreparedAdamInput {
    let request = options
        .backend_turn_request
        .as_ref()
        .expect("runner validates the shared backend request before prompt rendering");
    let instruction_layers = [
        InhousePromptLayer::System,
        InhousePromptLayer::Developer,
        InhousePromptLayer::ToolUsageProtocol,
        InhousePromptLayer::UserProjectContext,
    ];
    let instructions = request
        .prompt_envelope
        .blocks
        .iter()
        .filter(|block| instruction_layers.contains(&block.layer))
        .map(|block| format!("## {}\n{}", block.block_id, block.body_text.trim()))
        .collect::<Vec<_>>()
        .join("\n\n");
    let turn_augment = request
        .prompt_envelope
        .blocks
        .iter()
        .filter(|block| block.layer == InhousePromptLayer::TurnAugment)
        .map(|block| block.body_text.trim())
        .filter(|body| !body.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let current_input = extract_text_input(input);
    let fallback_current_input = if turn_augment.is_empty() {
        current_input
    } else {
        format!("{turn_augment}\n\nUser message:\n{current_input}")
    };
    let projection = if request.message_projection.segments.is_empty() {
        render_canonical_thread_projection(
            &options.canonical_thread_messages,
            request
                .message_projection
                .current_turn_message_id
                .as_deref(),
            &fallback_current_input,
            &instructions,
        )
    } else {
        PolarPromptProjection::from_segments(
            &instructions,
            request.message_projection.segments.clone(),
            request
                .message_projection
                .current_turn_message_id
                .as_deref(),
        )
    };
    let full_prompt = if turn_augment.is_empty() {
        projection.full_prompt.clone()
    } else {
        format!(
            "{}\n\n## turn-augment\n{}",
            projection.full_prompt,
            turn_augment.trim()
        )
    };
    PreparedAdamInput {
        instructions,
        stable_prefix: (!projection.rendered_prompt.trim().is_empty())
            .then(|| projection.rendered_prompt.clone()),
        input: full_prompt.clone(),
        full_prompt,
    }
}

fn render_canonical_thread_projection(
    messages: &[InhouseCanonicalThreadMessage],
    current_message_id: Option<&str>,
    current_input_text: &str,
    stable_instruction_prefix: &str,
) -> PolarPromptProjection {
    if messages.is_empty() {
        let segment = current_message_id
            .filter(|message_id| !message_id.trim().is_empty())
            .map(|message_id| {
                InhousePromptSegment::from_lgp_message(
                    message_id,
                    "user",
                    ADAM_LEGACY_CANONICAL_MESSAGE_REVISION,
                    current_input_text,
                    ADAM_CONTEXT_COMPACTION_EPOCH_UNKNOWN,
                )
            });
        return match segment {
            Some(segment) => PolarPromptProjection::from_segments(
                stable_instruction_prefix,
                vec![segment],
                current_message_id,
            ),
            None => PolarPromptProjection::from_segments(
                stable_instruction_prefix,
                vec![InhousePromptSegment::from_lgp_message(
                    "current",
                    "user",
                    ADAM_LEGACY_CANONICAL_MESSAGE_REVISION,
                    current_input_text,
                    ADAM_CONTEXT_COMPACTION_EPOCH_UNKNOWN,
                )],
                None,
            ),
        };
    }
    let mut segments = Vec::new();
    let mut saw_current = false;
    for message in messages {
        let is_current = current_message_id == Some(message.message_id.as_str());
        let text = if is_current {
            current_input_text
        } else {
            message.text.as_str()
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if is_current {
            saw_current = true;
        }
        segments.push(InhousePromptSegment::from_lgp_message(
            &message.message_id,
            &message.role,
            ADAM_LEGACY_CANONICAL_MESSAGE_REVISION,
            text,
            ADAM_CONTEXT_COMPACTION_EPOCH_UNKNOWN,
        ));
    }
    if !saw_current {
        let message_id = current_message_id.unwrap_or("current");
        segments.push(InhousePromptSegment::from_lgp_message(
            message_id,
            "user",
            ADAM_LEGACY_CANONICAL_MESSAGE_REVISION,
            current_input_text,
            ADAM_CONTEXT_COMPACTION_EPOCH_UNKNOWN,
        ));
    }
    PolarPromptProjection::from_segments(stable_instruction_prefix, segments, current_message_id)
}

fn prompt_cache_key(context: &AdamTurnContext) -> String {
    let mut hasher = Sha256::new();
    hasher.update(context.owner_id.as_bytes());
    hasher.update([0]);
    hasher.update(context.thread_id.as_bytes());
    hasher.update([0]);
    hasher.update(context.inhouse_polar_id.as_bytes());
    hasher.update([0]);
    hasher.update(context.backend_profile_id.as_bytes());
    let digest = hasher.finalize();
    format!(
        "adam_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
    )
}

fn extract_text_input(input: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(input) else {
        return input.to_string();
    };
    match value {
        Value::String(text) => text,
        Value::Object(map) => map
            .get("text")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| Value::Object(map).to_string()),
        other => other.to_string(),
    }
}

struct OpenAiCompatibleResponsesConnector {
    config: crate::config::LlamaConfig,
    http: reqwest::Client,
}

impl OpenAiCompatibleResponsesConnector {
    fn new(config: crate::config::LlamaConfig) -> Result<Self, AdamBackendError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.request_timeout_secs))
            .build()
            .map_err(|err| AdamBackendError::Transport(err.to_string()))?;
        Ok(Self { config, http })
    }
}

#[async_trait]
impl AdamProviderConnector for OpenAiCompatibleResponsesConnector {
    async fn run_turn(
        &self,
        mut request: AdamProviderRequest,
        sink: &mut dyn AdamProviderEventSink,
    ) -> Result<AdamProviderTurnEnd, AdamBackendError> {
        request
            .max_output_tokens
            .get_or_insert(self.config.max_output_tokens);
        let body = build_responses_request(&request, &self.config.model);
        let response = self
            .http
            .post(crate::append_path(&self.config.base_url, "responses"))
            .json(&body)
            .send()
            .await
            .map_err(|err| AdamBackendError::Transport(err.to_string()))?;
        let status = response.status();
        let value = response
            .json::<Value>()
            .await
            .map_err(|err| AdamBackendError::Parse(err.to_string()))?;
        if !status.is_success() {
            return Err(AdamBackendError::Provider {
                status: Some(status.as_u16()),
                message: value.to_string(),
            });
        }
        let parsed = parse_responses_response(&value);
        if !parsed.text.is_empty() {
            sink.emit(AdamProviderEvent::AnswerDelta(parsed.text))
                .await?;
        }
        for call in parsed.function_calls {
            sink.emit(AdamProviderEvent::ToolCall(call)).await?;
        }
        Ok(AdamProviderTurnEnd {
            provider_response_id: parsed.response_id,
            cache_evidence: openai_cache_evidence_from_usage(
                parsed.usage.as_ref(),
                "openai_compatible_responses",
                "openai_compatible_responses",
            ),
            usage: parsed.usage,
            raw: Some(parsed.raw),
        })
    }
}

struct OpenAiCompatibleChatConnector {
    config: crate::config::LlamaConfig,
    http: reqwest::Client,
}

impl OpenAiCompatibleChatConnector {
    fn new(config: crate::config::LlamaConfig) -> Result<Self, AdamBackendError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.request_timeout_secs))
            .build()
            .map_err(|err| AdamBackendError::Transport(err.to_string()))?;
        Ok(Self { config, http })
    }
}

#[async_trait]
impl AdamProviderConnector for OpenAiCompatibleChatConnector {
    async fn run_turn(
        &self,
        mut request: AdamProviderRequest,
        sink: &mut dyn AdamProviderEventSink,
    ) -> Result<AdamProviderTurnEnd, AdamBackendError> {
        request
            .max_output_tokens
            .get_or_insert(self.config.max_output_tokens);
        let body = build_chat_completions_request(&request, &self.config.model);
        let response = self
            .http
            .post(crate::append_path(
                &self.config.base_url,
                "chat/completions",
            ))
            .json(&body)
            .send()
            .await
            .map_err(|err| AdamBackendError::Transport(err.to_string()))?;
        let status = response.status();
        let value = response
            .json::<Value>()
            .await
            .map_err(|err| AdamBackendError::Parse(err.to_string()))?;
        if !status.is_success() {
            return Err(AdamBackendError::Provider {
                status: Some(status.as_u16()),
                message: value.to_string(),
            });
        }
        let parsed = parse_chat_completions_response(&value);
        if !parsed.text.is_empty() {
            sink.emit(AdamProviderEvent::AnswerDelta(parsed.text))
                .await?;
        }
        for call in parsed.function_calls {
            sink.emit(AdamProviderEvent::ToolCall(call)).await?;
        }
        Ok(AdamProviderTurnEnd {
            provider_response_id: parsed.response_id,
            cache_evidence: openai_cache_evidence_from_usage(
                parsed.usage.as_ref(),
                "openai_compatible_chat",
                "openai_compatible_chat",
            ),
            usage: parsed.usage,
            raw: Some(parsed.raw),
        })
    }
}


