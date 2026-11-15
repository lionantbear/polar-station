use crate::append_path;
use crate::config::LlamaConfig;
use crate::runner::{
    AdamBackendError, AdamBusyKind, AdamProviderConnector, AdamProviderEvent,
    AdamProviderEventSink, AdamProviderRequest, AdamProviderTurnEnd, AdamThinkingSource,
    AdamToolCall, AdamToolResult,
};
use async_trait::async_trait;
use base64::Engine as _;
use futures_util::StreamExt;
use polar_inhouse_contract::InhouseProviderCacheEvidence;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;

pub const LLAMACPP_HIGH_CACHE_HIT_RATE: f64 = 0.80;
pub const LLAMACPP_WIRE_PREFIX_PROBE_SUFFIX_A: &str = "\u{e000}";
pub const LLAMACPP_WIRE_PREFIX_PROBE_SUFFIX_B: &str = "\u{e001}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlamaSlotProbe {
    pub total_slots: u32,
    pub idle_slots: Vec<i64>,
    pub busy_slots: Vec<i64>,
    pub source_endpoint: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlamaSlotCacheSnapshot {
    pub endpoint_fingerprint: String,
    pub model_label: String,
    pub slot_id: i64,
    pub renderer_version: String,
    pub template_hash: String,
    pub tool_schema_hash: String,
    pub context_compaction_epoch: i64,
    pub exact_prefix_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlamaSlotCacheRequest {
    pub endpoint_fingerprint: String,
    pub model_label: String,
    pub slot_id: i64,
    pub renderer_version: String,
    pub template_hash: String,
    pub tool_schema_hash: String,
    pub context_compaction_epoch: i64,
    pub exact_prefix_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LlamaSlotReuseDecision {
    ReuseExactPrefixSlot { slot_id: i64 },
    Rewarm { slot_id: i64, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedCompactSlot {
    pub slot_id: i64,
    pub candidate_compaction_id: String,
    pub source_digest_before: String,
    pub checkpoint_source_ref: String,
    pub target_context_compaction_epoch: i64,
    pub exact_prefix_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedCompactSlotRequest {
    pub candidate_compaction_id: String,
    pub source_digest_before: String,
    pub checkpoint_source_ref: String,
    pub target_context_compaction_epoch: i64,
    pub exact_prefix_hash: String,
    pub active_slot_ids: Vec<i64>,
    pub idle_slot_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreparedCompactSlotDecision {
    Prepare { slot_id: i64 },
    Promote { slot_id: i64 },
    Discard { reason: String },
    Skip { reason: String },
}

impl LlamaSlotProbe {
    pub fn usable_slots(&self, slot_offset: u32, requested_count: Option<u32>) -> Vec<i64> {
        let total = self.total_slots.saturating_sub(slot_offset);
        let count = requested_count.unwrap_or(total).min(total);
        (slot_offset..slot_offset.saturating_add(count))
            .map(i64::from)
            .collect()
    }
}

pub fn decide_llama_slot_reuse(
    snapshot: Option<&LlamaSlotCacheSnapshot>,
    request: &LlamaSlotCacheRequest,
) -> LlamaSlotReuseDecision {
    let Some(snapshot) = snapshot else {
        return LlamaSlotReuseDecision::Rewarm {
            slot_id: request.slot_id,
            reason: "no_slot_cache_snapshot".to_string(),
        };
    };
    for (field, previous, current) in [
        (
            "endpoint_fingerprint",
            snapshot.endpoint_fingerprint.as_str(),
            request.endpoint_fingerprint.as_str(),
        ),
        (
            "model_label",
            snapshot.model_label.as_str(),
            request.model_label.as_str(),
        ),
        (
            "renderer_version",
            snapshot.renderer_version.as_str(),
            request.renderer_version.as_str(),
        ),
        (
            "template_hash",
            snapshot.template_hash.as_str(),
            request.template_hash.as_str(),
        ),
        (
            "tool_schema_hash",
            snapshot.tool_schema_hash.as_str(),
            request.tool_schema_hash.as_str(),
        ),
        (
            "exact_prefix_hash",
            snapshot.exact_prefix_hash.as_str(),
            request.exact_prefix_hash.as_str(),
        ),
    ] {
        if previous != current {
            return LlamaSlotReuseDecision::Rewarm {
                slot_id: request.slot_id,
                reason: format!("{field}_changed"),
            };
        }
    }
    if snapshot.slot_id != request.slot_id {
        return LlamaSlotReuseDecision::Rewarm {
            slot_id: request.slot_id,
            reason: "slot_id_changed".to_string(),
        };
    }
    if snapshot.context_compaction_epoch != request.context_compaction_epoch {
        return LlamaSlotReuseDecision::Rewarm {
            slot_id: request.slot_id,
            reason: "context_compaction_epoch_changed".to_string(),
        };
    }
    LlamaSlotReuseDecision::ReuseExactPrefixSlot {
        slot_id: request.slot_id,
    }
}

pub fn decide_prepared_compact_slot(
    prepared: Option<&PreparedCompactSlot>,
    request: &PreparedCompactSlotRequest,
) -> PreparedCompactSlotDecision {
    if let Some(prepared) = prepared {
        if request.active_slot_ids.contains(&prepared.slot_id) {
            return PreparedCompactSlotDecision::Discard {
                reason: "prepared_slot_became_active".to_string(),
            };
        }
        for (field, previous, current) in [
            (
                "candidate_compaction_id",
                prepared.candidate_compaction_id.as_str(),
                request.candidate_compaction_id.as_str(),
            ),
            (
                "source_digest_before",
                prepared.source_digest_before.as_str(),
                request.source_digest_before.as_str(),
            ),
            (
                "checkpoint_source_ref",
                prepared.checkpoint_source_ref.as_str(),
                request.checkpoint_source_ref.as_str(),
            ),
            (
                "exact_prefix_hash",
                prepared.exact_prefix_hash.as_str(),
                request.exact_prefix_hash.as_str(),
            ),
        ] {
            if previous != current {
                return PreparedCompactSlotDecision::Discard {
                    reason: format!("{field}_mismatch"),
                };
            }
        }
        if prepared.target_context_compaction_epoch != request.target_context_compaction_epoch {
            return PreparedCompactSlotDecision::Discard {
                reason: "target_context_compaction_epoch_mismatch".to_string(),
            };
        }
        if request.idle_slot_ids.contains(&prepared.slot_id) {
            return PreparedCompactSlotDecision::Promote {
                slot_id: prepared.slot_id,
            };
        }
        return PreparedCompactSlotDecision::Discard {
            reason: "prepared_slot_lost".to_string(),
        };
    }

    request
        .idle_slot_ids
        .iter()
        .copied()
        .find(|slot_id| !request.active_slot_ids.contains(slot_id))
        .map(|slot_id| PreparedCompactSlotDecision::Prepare { slot_id })
        .unwrap_or_else(|| PreparedCompactSlotDecision::Skip {
            reason: "no_spare_idle_slot".to_string(),
        })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlamaCompletionRequest {
    pub prompt: String,
    pub id_slot: i64,
    pub cache_prompt: bool,
    pub n_predict: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

/// Provider-neutral prompt projections that must be rendered into the exact
/// llama.cpp wire bytes used for KV identity and generation.
///
/// This request is intentionally limited to no-tool turns. Tool and grammar
/// turns stay on llama.cpp chat completions so its template-derived parser,
/// grammar, and stop metadata remain coupled to inference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlamaWireProjectionRequest {
    pub stable_projection: String,
    pub full_projection: String,
    pub enable_thinking: bool,
}

/// Exact rendered llama.cpp prompt text for cache warmup and generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlamaWireProjection {
    pub exact_prefix: String,
    pub full_prompt: String,
    /// True when the rendered generation prefill has opened `<think>` without
    /// closing it. Native completion deltas then begin inside thinking state.
    pub starts_in_thinking: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlamaCompletionOutput {
    pub content: String,
    pub slot_id: Option<i64>,
    pub cache_hit_tokens: Option<i64>,
    pub cache_miss_tokens: Option<i64>,
    pub provider_response_id: Option<String>,
    #[serde(default)]
    pub streamed_text: bool,
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenAiResponseOutput {
    pub response_id: Option<String>,
    pub text: String,
    pub function_calls: Vec<AdamToolCall>,
    pub usage: Option<Value>,
    pub cache_evidence: Option<Value>,
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatCompletionOutput {
    pub response_id: Option<String>,
    pub text: String,
    pub function_calls: Vec<AdamToolCall>,
    pub usage: Option<Value>,
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdamLocalToolEnvelopeAction {
    FinalText {
        text: String,
    },
    ToolCall {
        call_id: String,
        name: String,
        arguments: Value,
    },
}

#[derive(Debug, Clone)]
pub struct LlamaCppSlotsClient {
    config: LlamaConfig,
    http: reqwest::Client,
    streaming_http: reqwest::Client,
}

impl LlamaCppSlotsClient {
    pub fn new(config: LlamaConfig) -> Result<Self, AdamBackendError> {
        let request_timeout = Duration::from_secs(config.request_timeout_secs);
        let http = reqwest::Client::builder()
            .timeout(request_timeout)
            .build()
            .map_err(|err| AdamBackendError::Transport(err.to_string()))?;
        let streaming_http = reqwest::Client::builder()
            .connect_timeout(request_timeout)
            .build()
            .map_err(|err| AdamBackendError::Transport(err.to_string()))?;
        Ok(Self {
            config,
            http,
            streaming_http,
        })
    }

    pub fn config(&self) -> &LlamaConfig {
        &self.config
    }

    /// Render the shared no-tool projection into the exact prompt text that
    /// llama.cpp will consume. Template application is authoritative: any HTTP,
    /// response-shape, semantic-prefix, or rendered-prefix mismatch fails the
    /// turn without falling back to raw `/completion` input.
    pub async fn project_wire_prompt(
        &self,
        request: &LlamaWireProjectionRequest,
    ) -> Result<LlamaWireProjection, AdamBackendError> {
        let stable_a = format!(
            "{}{}",
            request.stable_projection, LLAMACPP_WIRE_PREFIX_PROBE_SUFFIX_A
        );
        let stable_b = format!(
            "{}{}",
            request.stable_projection, LLAMACPP_WIRE_PREFIX_PROBE_SUFFIX_B
        );
        let rendered_stable_a = self
            .apply_template(&stable_a, request.enable_thinking)
            .await?;
        let rendered_stable_b = self
            .apply_template(&stable_b, request.enable_thinking)
            .await?;
        let rendered_full = self
            .apply_template(&request.full_projection, request.enable_thinking)
            .await?;
        derive_llamacpp_wire_projection(
            request,
            &rendered_stable_a,
            &rendered_stable_b,
            &rendered_full,
        )
    }

    async fn apply_template(
        &self,
        projection: &str,
        enable_thinking: bool,
    ) -> Result<String, AdamBackendError> {
        let response = self
            .http
            .post(append_path(&self.config.base_url, "apply-template"))
            .json(&build_llamacpp_apply_template_body(
                projection,
                enable_thinking,
            ))
            .send()
            .await
            .map_err(|err| AdamBackendError::Transport(err.to_string()))?;
        let status = response.status();
        let value = response_json_or_error(status, response).await;
        if !status.is_success() {
            return Err(AdamBackendError::Provider {
                status: Some(status.as_u16()),
                message: provider_error_message(&value).unwrap_or_else(|| value.to_string()),
            });
        }
        value
            .get("prompt")
            .and_then(Value::as_str)
            .filter(|prompt| !prompt.is_empty())
            .map(ToString::to_string)
            .ok_or_else(|| {
                AdamBackendError::Parse(
                    "llama.cpp /apply-template response did not contain a non-empty string `prompt`"
                        .to_string(),
                )
            })
    }

    pub async fn probe_slots(&self) -> Result<LlamaSlotProbe, AdamBackendError> {
        let slots_url = append_path(&self.config.base_url, "slots");
        match self.http.get(&slots_url).send().await {
            Ok(response) if response.status().is_success() => {
                let value = response
                    .json::<Value>()
                    .await
                    .map_err(|err| AdamBackendError::Parse(err.to_string()))?;
                if let Some(probe) = parse_llamacpp_slots_response(&value) {
                    return Ok(probe);
                }
            }
            Ok(_) | Err(_) => {}
        }

        let props_url = append_path(&self.config.base_url, "props");
        let response = self
            .http
            .get(&props_url)
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
        parse_llamacpp_props_response(&value).ok_or_else(|| {
            AdamBackendError::Parse(
                "llama.cpp /props response did not expose slot count".to_string(),
            )
        })
    }

    pub async fn completion(
        &self,
        request: &LlamaCompletionRequest,
    ) -> Result<LlamaCompletionOutput, AdamBackendError> {
        let url = append_path(&self.config.base_url, "completion");
        let response = self
            .http
            .post(url)
            .json(request)
            .send()
            .await
            .map_err(|err| AdamBackendError::Transport(err.to_string()))?;
        let status = response.status();
        let value = response
            .json::<Value>()
            .await
            .unwrap_or_else(|_| json!({ "error": status.as_str() }));

        if classify_llamacpp_busy(status, &value) {
            return Err(AdamBackendError::ProviderBusy {
                kind: AdamBusyKind::LlamaProviderBusy,
                message: provider_error_message(&value)
                    .unwrap_or_else(|| format!("llama.cpp provider busy: HTTP {status}")),
            });
        }
        if !status.is_success() {
            return Err(AdamBackendError::Provider {
                status: Some(status.as_u16()),
                message: provider_error_message(&value).unwrap_or_else(|| value.to_string()),
            });
        }
        parse_llamacpp_completion_response(&value)
    }

    pub async fn completion_streaming(
        &self,
        request: &LlamaCompletionRequest,
        sink: &mut dyn AdamProviderEventSink,
    ) -> Result<LlamaCompletionOutput, AdamBackendError> {
        self.completion_streaming_with_initial_thinking(request, sink, false)
            .await
    }

    async fn completion_streaming_with_initial_thinking(
        &self,
        request: &LlamaCompletionRequest,
        sink: &mut dyn AdamProviderEventSink,
        starts_in_thinking: bool,
    ) -> Result<LlamaCompletionOutput, AdamBackendError> {
        let url = append_path(&self.config.base_url, "completion");
        let mut request = request.clone();
        request.stream = Some(true);
        let progress_timeout = Duration::from_secs(self.config.request_timeout_secs);
        let response = tokio::time::timeout(
            progress_timeout,
            self.streaming_http
                .post(url)
                .header("accept", "text/event-stream")
                .json(&request)
                .send(),
        )
        .await
        .map_err(|_| {
            AdamBackendError::Transport(format!(
                "llama.cpp streaming response made no progress for {}s while waiting for headers",
                self.config.request_timeout_secs
            ))
        })?
        .map_err(|err| AdamBackendError::Transport(err.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let value = tokio::time::timeout(
                progress_timeout,
                response_json_or_error(status, response),
            )
            .await
            .map_err(|_| {
                AdamBackendError::Transport(format!(
                    "llama.cpp streaming response made no progress for {}s while reading HTTP {status} error body",
                    self.config.request_timeout_secs
                ))
            })?;
            if classify_llamacpp_busy(status, &value) {
                return Err(AdamBackendError::ProviderBusy {
                    kind: AdamBusyKind::LlamaProviderBusy,
                    message: provider_error_message(&value)
                        .unwrap_or_else(|| format!("llama.cpp provider busy: HTTP {status}")),
                });
            }
            return Err(AdamBackendError::Provider {
                status: Some(status.as_u16()),
                message: provider_error_message(&value).unwrap_or_else(|| value.to_string()),
            });
        }

        let mut chunks = response.bytes_stream();
        let mut line_buffer = Vec::<u8>::new();
        let mut raw_events = Vec::<Value>::new();
        let mut content = String::new();
        let mut streamed_text = false;
        let mut think_parser = LlamaThinkTagParser::with_initial_thinking(starts_in_thinking);

        loop {
            let next_chunk = tokio::time::timeout(progress_timeout, chunks.next())
                .await
                .map_err(|_| {
                    AdamBackendError::Transport(format!(
                        "llama.cpp streaming response made no progress for {}s while waiting for the next body chunk",
                        self.config.request_timeout_secs
                    ))
                })?;
            let Some(chunk) = next_chunk else {
                break;
            };
            let chunk = chunk.map_err(|err| AdamBackendError::Transport(err.to_string()))?;
            line_buffer.extend_from_slice(&chunk);
            drain_llamacpp_stream_lines(
                &mut line_buffer,
                &mut raw_events,
                &mut content,
                sink,
                &mut think_parser,
                &mut streamed_text,
            )
            .await?;
        }
        if !line_buffer.is_empty() {
            let line = std::mem::take(&mut line_buffer);
            process_llamacpp_stream_line(
                &line,
                &mut raw_events,
                &mut content,
                sink,
                &mut think_parser,
                &mut streamed_text,
            )
            .await?;
        }
        think_parser.finish(sink).await?;

        Ok(llamacpp_streaming_output(
            content,
            raw_events,
            streamed_text,
        ))
    }

    async fn tool_chat_completion(
        &self,
        mut request: AdamProviderRequest,
        slot_id: i64,
    ) -> Result<ChatCompletionOutput, AdamBackendError> {
        request
            .max_output_tokens
            .get_or_insert(self.config.max_output_tokens);
        let mut body = build_chat_completions_request(&request, &self.config.model);
        body["id_slot"] = json!(slot_id);
        body["cache_prompt"] = json!(self.config.cache_prompt);
        // Tool arguments are protocol data, not creative prose. Keep llama's
        // constrained tool-call turn deterministic so identifiers and exact
        // user-provided strings survive the adapter boundary byte-for-byte.
        body["temperature"] = json!(0.0);
        if self.config.llama_require_tool_call && request.tool_results.is_empty() {
            body["tool_choice"] = json!("required");
            if let Some(tools) = body["tools"].as_array_mut() {
                for tool in tools {
                    let Some(parameters) = tool
                        .get_mut("function")
                        .and_then(|function| function.get_mut("parameters"))
                    else {
                        continue;
                    };
                    let Some(properties) = parameters.get("properties").and_then(Value::as_object)
                    else {
                        continue;
                    };
                    let required = properties.keys().cloned().map(Value::String).collect();
                    parameters["required"] = Value::Array(required);
                }
            }
        }
        body["chat_template_kwargs"] = json!({
            "enable_thinking": self.config.llama_enable_thinking,
        });
        let response = self
            .http
            .post(append_path(&self.config.base_url, "v1/chat/completions"))
            .json(&body)
            .send()
            .await
            .map_err(|error| AdamBackendError::Transport(error.to_string()))?;
        let status = response.status();
        let value = response_json_or_error(status, response).await;
        if classify_llamacpp_busy(status, &value) {
            return Err(AdamBackendError::ProviderBusy {
                kind: AdamBusyKind::LlamaProviderBusy,
                message: provider_error_message(&value)
                    .unwrap_or_else(|| format!("llama.cpp provider busy: HTTP {status}")),
            });
        }
        if !status.is_success() {
            return Err(AdamBackendError::Provider {
                status: Some(status.as_u16()),
                message: provider_error_message(&value).unwrap_or_else(|| value.to_string()),
            });
        }
        Ok(parse_chat_completions_response(&value))
    }
}

#[async_trait]
impl AdamProviderConnector for LlamaCppSlotsClient {
    async fn project_no_tool_wire_prompt(
        &self,
        stable_projection: &str,
        full_projection: &str,
    ) -> Result<Option<LlamaWireProjection>, AdamBackendError> {
        self.project_wire_prompt(&LlamaWireProjectionRequest {
            stable_projection: stable_projection.to_string(),
            full_projection: full_projection.to_string(),
            enable_thinking: self.config.llama_enable_thinking,
        })
        .await
        .map(Some)
    }

    async fn run_turn(
        &self,
        request: AdamProviderRequest,
        sink: &mut dyn AdamProviderEventSink,
    ) -> Result<AdamProviderTurnEnd, AdamBackendError> {
        let slot_id = select_llamacpp_slot(&self.config, &request);
        if !request.tools.is_empty() || !request.tool_results.is_empty() {
            let output = self.tool_chat_completion(request, slot_id).await?;
            if !output.text.is_empty() {
                emit_llama_text_events(&output.text, sink).await?;
            }
            for call in output.function_calls {
                sink.emit(AdamProviderEvent::ToolCall(call)).await?;
            }
            let cache_evidence = llamacpp_chat_cache_evidence(
                slot_id,
                output.usage.as_ref(),
                output.raw.clone(),
                None,
            );
            return Ok(AdamProviderTurnEnd {
                provider_response_id: output.response_id,
                usage: output.usage,
                cache_evidence: Some(cache_evidence),
                raw: Some(output.raw),
            });
        }
        let wire = match request.wire_projection.clone() {
            Some(wire) => wire,
            None => {
                self.project_wire_prompt(&LlamaWireProjectionRequest {
                    stable_projection: request.stable_prefix.clone().unwrap_or_default(),
                    full_projection: request.input.clone(),
                    enable_thinking: self.config.llama_enable_thinking,
                })
                .await?
            }
        };
        let warmup = build_llamacpp_completion_request(
            &wire.exact_prefix,
            slot_id,
            0,
            self.config.cache_prompt,
            None,
            None,
        );
        let warmup_output = Some(self.completion(&warmup).await?);
        let prompt = wire.full_prompt;
        let starts_in_thinking = wire.starts_in_thinking;
        let n_predict = request
            .max_output_tokens
            .unwrap_or(self.config.max_output_tokens);
        tracing::info!(
            slot_id,
            prompt_len = prompt.len(),
            trimmed_prompt_len = prompt.trim().len(),
            stable_prefix_len = request.stable_prefix.as_deref().map(str::len).unwrap_or(0),
            tool_count = request.tools.len(),
            tool_result_count = request.tool_results.len(),
            n_predict,
            "adam.llama.completion_request_ready"
        );
        let completion = build_llamacpp_completion_request(
            &prompt,
            slot_id,
            n_predict,
            self.config.cache_prompt,
            None,
            None,
        );
        let can_stream_text = request.tools.is_empty();
        let output = match can_stream_text {
            true => {
                self.completion_streaming_with_initial_thinking(
                    &completion,
                    sink,
                    starts_in_thinking,
                )
                .await?
            }
            false => self.completion(&completion).await?,
        };
        tracing::info!(
            slot_id,
            content_len = output.content.len(),
            trimmed_content_len = output.content.trim().len(),
            cache_hit_tokens = output.cache_hit_tokens,
            cache_miss_tokens = output.cache_miss_tokens,
            "adam.llama.completion_returned"
        );
        if !output.streamed_text {
            let envelope = parse_strict_tool_envelope(&output.content)?;
            match envelope {
                Some(AdamLocalToolEnvelopeAction::FinalText { text }) => {
                    emit_llama_text_events(&text, sink).await?;
                }
                Some(AdamLocalToolEnvelopeAction::ToolCall {
                    call_id,
                    name,
                    arguments,
                }) => {
                    sink.emit(AdamProviderEvent::ToolCall(AdamToolCall {
                        provider_call_id: Some(call_id.clone()),
                        call_id,
                        name,
                        arguments,
                    }))
                    .await?;
                }
                None => emit_llama_text_events(&output.content, sink).await?,
            }
        }

        Ok(AdamProviderTurnEnd {
            provider_response_id: output.provider_response_id,
            usage: None,
            cache_evidence: Some(llamacpp_slots_cache_evidence(
                output.slot_id.unwrap_or(slot_id),
                output.cache_hit_tokens,
                output.cache_miss_tokens,
                output.raw.clone(),
                warmup_output.as_ref(),
            )),
            raw: Some(output.raw),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct LlamaCacheStats {
    cache_read_tokens: i64,
    input_uncached_tokens: i64,
    cache_hit_rate: Option<f64>,
}

impl LlamaCacheStats {
    fn new(cache_read_tokens: Option<i64>, input_uncached_tokens: Option<i64>) -> Self {
        let cache_read_tokens = cache_read_tokens.unwrap_or(0).max(0);
        let input_uncached_tokens = input_uncached_tokens.unwrap_or(0).max(0);
        let denominator = cache_read_tokens + input_uncached_tokens;
        let cache_hit_rate =
            (denominator > 0).then(|| cache_read_tokens as f64 / denominator as f64);
        Self {
            cache_read_tokens,
            input_uncached_tokens,
            cache_hit_rate,
        }
    }

    fn high_cache_hit(self) -> bool {
        self.cache_hit_rate
            .is_some_and(|rate| rate >= LLAMACPP_HIGH_CACHE_HIT_RATE)
    }

    fn as_json(self, raw: &Value) -> Value {
        json!({
            "cache_read_tokens": self.cache_read_tokens,
            "input_uncached_tokens": self.input_uncached_tokens,
            "cache_hit_rate": self.cache_hit_rate,
            "cache_hit_rate_numerator": self.cache_read_tokens,
            "cache_hit_rate_denominator": self.cache_read_tokens + self.input_uncached_tokens,
            "high_cache_hit_threshold": LLAMACPP_HIGH_CACHE_HIT_RATE,
            "provider_metadata": safe_llamacpp_provider_metadata(raw),
        })
    }
}

/// Cache evidence is LGP/Diagnostics-visible. Preserve only typed provider
/// metadata needed to explain cache accounting; raw completion events can
/// contain answer or thinking text and must never cross that boundary.
fn safe_llamacpp_provider_metadata(raw: &Value) -> Value {
    let events = raw.get("events").and_then(Value::as_array);
    let terminal = events
        .and_then(|events| events.iter().rev().find(|event| event.is_object()))
        .unwrap_or(raw);
    let event_count =
        events.map_or_else(|| u64::from(raw.is_object()), |events| events.len() as u64);
    json!({
        "shape": if events.is_some() { "stream_event_summary" } else { "completion_summary" },
        "event_count": event_count,
        "slot_id": terminal
            .get("slot_id")
            .or_else(|| terminal.get("id_slot"))
            .and_then(Value::as_i64),
        "cache_hit_tokens": terminal
            .get("cache_hit_tokens")
            .or_else(|| terminal.pointer("/timings/prompt_cache_hit_tokens"))
            .or_else(|| terminal.pointer("/timings/cache_n"))
            .and_then(Value::as_i64),
        "cache_miss_tokens": terminal
            .get("cache_miss_tokens")
            .or_else(|| terminal.pointer("/timings/prompt_n"))
            .and_then(Value::as_i64),
        "output_tokens": llamacpp_output_tokens(raw),
        "provider_response_id_present": terminal
            .get("id")
            .and_then(Value::as_str)
            .is_some(),
    })
}

fn llamacpp_slots_cache_evidence(
    slot_id: i64,
    cache_hit_tokens: Option<i64>,
    cache_miss_tokens: Option<i64>,
    raw: Value,
    warmup_output: Option<&LlamaCompletionOutput>,
) -> InhouseProviderCacheEvidence {
    let completion_stats = LlamaCacheStats::new(cache_hit_tokens, cache_miss_tokens);
    let warmup_stats = warmup_output
        .map(|output| LlamaCacheStats::new(output.cache_hit_tokens, output.cache_miss_tokens));
    let selected_source = match warmup_stats {
        Some(warmup) if warmup.high_cache_hit() => "warmup",
        _ if completion_stats.high_cache_hit() => "completion",
        Some(warmup) if warmup.cache_read_tokens > completion_stats.cache_read_tokens => "warmup",
        _ => "completion",
    };
    let selected_stats = match (selected_source, warmup_stats) {
        ("warmup", Some(warmup)) => warmup,
        _ => completion_stats,
    };
    let output_tokens = llamacpp_output_tokens(&raw);
    InhouseProviderCacheEvidence {
        backend: "llama".to_string(),
        wire_family: "llamacpp_completion".to_string(),
        dialect: "llamacpp_slots".to_string(),
        evidence_source: "runtime_control_slot_kv".to_string(),
        input_uncached_tokens: selected_stats.input_uncached_tokens,
        cache_read_tokens: selected_stats.cache_read_tokens,
        cache_write_tokens: 0,
        output_tokens,
        cache_hit_rate: selected_stats.cache_hit_rate,
        cache_hit: selected_stats.high_cache_hit(),
        required_cache_satisfied: Some(selected_stats.high_cache_hit()),
        provider_slot_id: Some(slot_id),
        details: Some(json!({
            "slot_id": slot_id,
            "selected_cache_source": selected_source,
            "high_cache_hit_threshold": LLAMACPP_HIGH_CACHE_HIT_RATE,
            "completion": completion_stats.as_json(&raw),
            "warmup": warmup_output.map(|output| {
                LlamaCacheStats::new(output.cache_hit_tokens, output.cache_miss_tokens)
                    .as_json(&output.raw)
            }),
        })),
    }
}

fn llamacpp_chat_cache_evidence(
    slot_id: i64,
    usage: Option<&Value>,
    raw: Value,
    warmup_output: Option<&LlamaCompletionOutput>,
) -> InhouseProviderCacheEvidence {
    let cached_tokens = usage
        .and_then(|usage| {
            usage
                .pointer("/prompt_tokens_details/cached_tokens")
                .or_else(|| usage.pointer("/timings/prompt_cache_hit_tokens"))
                .or_else(|| usage.get("cached_tokens"))
        })
        .and_then(Value::as_i64)
        .or_else(|| {
            raw.pointer("/timings/prompt_cache_hit_tokens")
                .or_else(|| raw.pointer("/timings/cache_n"))
                .and_then(Value::as_i64)
        });
    let prompt_tokens = usage
        .and_then(|usage| {
            usage
                .get("prompt_tokens")
                .or_else(|| usage.get("input_tokens"))
                .or_else(|| usage.pointer("/timings/prompt_n"))
        })
        .and_then(Value::as_i64)
        .or_else(|| raw.pointer("/timings/prompt_n").and_then(Value::as_i64));
    let cache_miss_tokens = prompt_tokens.map(|total| {
        total
            .saturating_sub(cached_tokens.unwrap_or_default())
            .max(0)
    });
    let mut evidence = llamacpp_slots_cache_evidence(
        slot_id,
        cached_tokens,
        cache_miss_tokens,
        raw,
        warmup_output,
    );
    evidence.wire_family = "llamacpp_chat_completions".to_string();
    evidence.details = Some(json!({
        "slot_id": slot_id,
        "requested_slot_is_authoritative": true,
        "usage": usage,
        "warmup": warmup_output.map(|output| output.raw.clone())
    }));
    evidence
}

fn llamacpp_output_tokens(raw: &Value) -> Option<i64> {
    let metadata = raw
        .get("events")
        .and_then(Value::as_array)
        .and_then(|events| events.iter().rev().find(|event| event.is_object()))
        .unwrap_or(raw);
    metadata
        .get("tokens_predicted")
        .or_else(|| metadata.pointer("/timings/predicted_n"))
        .or_else(|| metadata.get("output_tokens"))
        .and_then(Value::as_i64)
}

async fn emit_llama_text_events(
    text: &str,
    sink: &mut dyn AdamProviderEventSink,
) -> Result<(), AdamBackendError> {
    let mut parser = LlamaThinkTagParser::new();
    parser.push(text, sink).await?;
    parser.finish(sink).await
}

#[derive(Debug, Default)]
pub struct LlamaThinkTagParser {
    buffer: String,
    in_think: bool,
}

impl LlamaThinkTagParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_initial_thinking(in_think: bool) -> Self {
        Self {
            buffer: String::new(),
            in_think,
        }
    }

    pub async fn push(
        &mut self,
        text: &str,
        sink: &mut dyn AdamProviderEventSink,
    ) -> Result<(), AdamBackendError> {
        self.buffer.push_str(text);
        loop {
            let tag = if self.in_think { "</think>" } else { "<think>" };
            if let Some(index) = self.buffer.find(tag) {
                self.emit_prefix(index, sink).await?;
                self.buffer.drain(..tag.len());
                if self.in_think {
                    sink.emit(AdamProviderEvent::ThinkingDelta {
                        text: String::new(),
                        source: AdamThinkingSource::LlamaThinkTag,
                        closed: Some(true),
                    })
                    .await?;
                    self.in_think = false;
                } else {
                    self.in_think = true;
                }
                continue;
            }

            let keep_len = longest_tag_prefix_suffix(&self.buffer, tag);
            let emit_len = self.buffer.len().saturating_sub(keep_len);
            if emit_len > 0 {
                self.emit_prefix(emit_len, sink).await?;
            }
            break;
        }
        Ok(())
    }

    pub async fn finish(
        &mut self,
        sink: &mut dyn AdamProviderEventSink,
    ) -> Result<(), AdamBackendError> {
        if !self.buffer.is_empty() {
            let len = self.buffer.len();
            self.emit_prefix(len, sink).await?;
        }
        if self.in_think {
            sink.emit(AdamProviderEvent::ThinkingDelta {
                text: String::new(),
                source: AdamThinkingSource::LlamaThinkTag,
                closed: Some(false),
            })
            .await?;
            self.in_think = false;
        }
        Ok(())
    }

    async fn emit_prefix(
        &mut self,
        len: usize,
        sink: &mut dyn AdamProviderEventSink,
    ) -> Result<(), AdamBackendError> {
        let text = self.buffer.drain(..len).collect::<String>();
        if text.is_empty() {
            return Ok(());
        }
        let event = if self.in_think {
            AdamProviderEvent::ThinkingDelta {
                text,
                source: AdamThinkingSource::LlamaThinkTag,
                closed: None,
            }
        } else {
            AdamProviderEvent::AnswerDelta(text)
        };
        sink.emit(event).await
    }
}

fn longest_tag_prefix_suffix(buffer: &str, tag: &str) -> usize {
    let max = buffer.len().min(tag.len().saturating_sub(1));
    (1..=max)
        .rev()
        .find(|len| buffer.ends_with(&tag[..*len]))
        .unwrap_or(0)
}

fn select_llamacpp_slot(config: &LlamaConfig, request: &AdamProviderRequest) -> i64 {
    if let Some(slot_id) = request.provider_slot_id {
        return slot_id;
    }

    let slot_count = config.slot_count.unwrap_or(1).max(1);
    let slot_index = match config.slot_strategy {
        crate::config::SlotStrategy::ThreadBound => {
            let mut hasher = Sha256::new();
            hasher.update(request.turn_context.owner_id.as_bytes());
            hasher.update([0]);
            hasher.update(request.turn_context.thread_id.as_bytes());
            hasher.update([0]);
            hasher.update(request.turn_context.inhouse_polar_id.as_bytes());
            hasher.update([0]);
            hasher.update(request.turn_context.backend_profile_id.as_bytes());
            let digest = hasher.finalize();
            let mut bytes = [0_u8; 8];
            bytes.copy_from_slice(&digest[..8]);
            (u64::from_be_bytes(bytes) % u64::from(slot_count)) as u32
        }
    };
    i64::from(config.slot_offset.saturating_add(slot_index))
}

/// Build the llama.cpp `/apply-template` body for a provider-neutral shared
/// projection. `enable_thinking` is deliberately serialized as a JSON boolean,
/// because llama.cpp rejects string-valued template control.
pub fn build_llamacpp_apply_template_body(projection: &str, enable_thinking: bool) -> Value {
    json!({
        "messages": [{
            "role": "user",
            "content": projection,
        }],
        "add_generation_prompt": true,
        "chat_template_kwargs": {
            "enable_thinking": enable_thinking,
        },
    })
}

/// Return the longest shared prefix without splitting a UTF-8 scalar value.
pub fn utf8_longest_common_prefix<'a>(left: &'a str, right: &str) -> &'a str {
    let mut prefix_len = 0;
    for ((left_index, left_char), (_, right_char)) in left.char_indices().zip(right.char_indices())
    {
        if left_char != right_char {
            break;
        }
        prefix_len = left_index + left_char.len_utf8();
    }
    &left[..prefix_len]
}

/// Derive and validate the exact llama.cpp wire projection from three
/// authoritative `/apply-template` results.
pub fn derive_llamacpp_wire_projection(
    request: &LlamaWireProjectionRequest,
    rendered_stable_a: &str,
    rendered_stable_b: &str,
    rendered_full: &str,
) -> Result<LlamaWireProjection, AdamBackendError> {
    if !request
        .full_projection
        .starts_with(&request.stable_projection)
    {
        return Err(AdamBackendError::Parse(
            "llama.cpp wire projection full prompt does not preserve the semantic stable prefix"
                .to_string(),
        ));
    }
    let exact_prefix = utf8_longest_common_prefix(rendered_stable_a, rendered_stable_b);
    if exact_prefix.is_empty() {
        return Err(AdamBackendError::Parse(
            "llama.cpp template renders did not expose a non-empty exact wire prefix".to_string(),
        ));
    }
    if !rendered_full.starts_with(exact_prefix) {
        return Err(AdamBackendError::Parse(
            "llama.cpp rendered full prompt does not preserve the exact wire prefix".to_string(),
        ));
    }
    Ok(LlamaWireProjection {
        exact_prefix: exact_prefix.to_string(),
        full_prompt: rendered_full.to_string(),
        starts_in_thinking: prompt_prefills_open_think(rendered_full),
    })
}

/// Hash exact llama.cpp wire prompt text using the cache-identity convention
/// used by Adam projections (SHA-256, URL-safe base64, no padding).
pub fn llamacpp_wire_prompt_hash(prompt: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prompt.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize())
}

fn prompt_prefills_open_think(prompt: &str) -> bool {
    let Some(open) = prompt.rfind("<think>") else {
        return false;
    };
    if prompt.rfind("</think>").is_some_and(|close| close > open) {
        return false;
    }
    prompt[open + "<think>".len()..].trim().is_empty()
}

pub fn build_llamacpp_completion_request(
    prompt: &str,
    id_slot: i64,
    n_predict: i64,
    cache_prompt: bool,
    temperature: Option<f32>,
    stop: Option<Vec<String>>,
) -> LlamaCompletionRequest {
    LlamaCompletionRequest {
        prompt: prompt.to_string(),
        id_slot,
        cache_prompt,
        n_predict,
        temperature,
        stop,
        stream: None,
    }
}

async fn response_json_or_error(status: StatusCode, response: reqwest::Response) -> Value {
    let text = response
        .text()
        .await
        .unwrap_or_else(|_| status.as_str().to_string());
    serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({ "error": text }))
}

async fn drain_llamacpp_stream_lines(
    line_buffer: &mut Vec<u8>,
    raw_events: &mut Vec<Value>,
    content: &mut String,
    sink: &mut dyn AdamProviderEventSink,
    think_parser: &mut LlamaThinkTagParser,
    streamed_text: &mut bool,
) -> Result<(), AdamBackendError> {
    while let Some(index) = line_buffer.iter().position(|byte| *byte == b'\n') {
        let line = line_buffer.drain(..=index).collect::<Vec<_>>();
        process_llamacpp_stream_line(
            &line,
            raw_events,
            content,
            sink,
            think_parser,
            streamed_text,
        )
        .await?;
    }
    Ok(())
}

async fn process_llamacpp_stream_line(
    line: &[u8],
    raw_events: &mut Vec<Value>,
    content: &mut String,
    sink: &mut dyn AdamProviderEventSink,
    think_parser: &mut LlamaThinkTagParser,
    streamed_text: &mut bool,
) -> Result<(), AdamBackendError> {
    let line = std::str::from_utf8(line)
        .map_err(|err| AdamBackendError::Parse(format!("invalid llama.cpp stream UTF-8: {err}")))?;
    let Some(value) = parse_llamacpp_stream_line(line)? else {
        return Ok(());
    };
    if let Some(delta) = llamacpp_stream_text_delta(&value).filter(|delta| !delta.is_empty()) {
        content.push_str(delta);
        think_parser.push(delta, sink).await?;
        *streamed_text = true;
    }
    raw_events.push(value);
    Ok(())
}

fn parse_llamacpp_stream_line(line: &str) -> Result<Option<Value>, AdamBackendError> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with(':') || trimmed.starts_with("event:") {
        return Ok(None);
    }
    let payload = trimmed
        .strip_prefix("data:")
        .map(str::trim)
        .unwrap_or(trimmed);
    if payload.is_empty() || payload == "[DONE]" {
        return Ok(None);
    }
    serde_json::from_str::<Value>(payload)
        .map(Some)
        .map_err(|err| AdamBackendError::Parse(format!("parse llama.cpp stream event: {err}")))
}

fn llamacpp_stream_text_delta(value: &Value) -> Option<&str> {
    value
        .get("content")
        .or_else(|| value.get("text"))
        .or_else(|| value.get("completion"))
        .or_else(|| value.pointer("/choices/0/text"))
        .or_else(|| value.pointer("/choices/0/delta/content"))
        .and_then(Value::as_str)
}

fn llamacpp_streaming_output(
    content: String,
    raw_events: Vec<Value>,
    streamed_text: bool,
) -> LlamaCompletionOutput {
    let metadata = raw_events.iter().rev().find(|event| event.is_object());
    LlamaCompletionOutput {
        content,
        slot_id: metadata
            .and_then(|event| event.get("slot_id").or_else(|| event.get("id_slot")))
            .and_then(Value::as_i64),
        cache_hit_tokens: metadata
            .and_then(|event| {
                event
                    .get("cache_hit_tokens")
                    .or_else(|| event.pointer("/timings/prompt_cache_hit_tokens"))
                    .or_else(|| event.pointer("/timings/cache_n"))
                    .or_else(|| event.get("tokens_cached"))
            })
            .and_then(Value::as_i64),
        cache_miss_tokens: metadata
            .and_then(|event| {
                event
                    .get("cache_miss_tokens")
                    .or_else(|| event.pointer("/timings/prompt_cache_miss_tokens"))
                    .or_else(|| event.pointer("/timings/prompt_n"))
            })
            .and_then(Value::as_i64),
        provider_response_id: metadata
            .and_then(|event| event.get("id"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        streamed_text,
        raw: json!({
            "transport": "llamacpp_slots_streaming",
            "events": raw_events
        }),
    }
}

pub fn parse_llamacpp_slots_response(value: &Value) -> Option<LlamaSlotProbe> {
    let slots = value
        .as_array()
        .or_else(|| value.get("slots").and_then(Value::as_array))?;
    let mut idle_slots = Vec::new();
    let mut busy_slots = Vec::new();
    for (index, slot) in slots.iter().enumerate() {
        let id = slot
            .get("id")
            .or_else(|| slot.get("id_slot"))
            .and_then(Value::as_i64)
            .unwrap_or_else(|| i64::try_from(index).unwrap_or(i64::MAX));
        let processing = slot
            .get("is_processing")
            .or_else(|| slot.get("processing"))
            .or_else(|| slot.get("busy"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if processing {
            busy_slots.push(id);
        } else {
            idle_slots.push(id);
        }
    }
    Some(LlamaSlotProbe {
        total_slots: u32::try_from(slots.len()).ok()?,
        idle_slots,
        busy_slots,
        source_endpoint: "/slots",
    })
}

pub fn parse_llamacpp_props_response(value: &Value) -> Option<LlamaSlotProbe> {
    let total = value
        .get("total_slots")
        .or_else(|| value.get("n_slots"))
        .or_else(|| value.pointer("/default_generation_settings/n_slots"))
        .or_else(|| value.pointer("/server/slots"))
        .and_then(Value::as_u64)?;
    let total_slots = u32::try_from(total).ok()?;
    Some(LlamaSlotProbe {
        total_slots,
        idle_slots: (0..total_slots).map(i64::from).collect(),
        busy_slots: Vec::new(),
        source_endpoint: "/props",
    })
}

pub fn parse_llamacpp_completion_response(
    value: &Value,
) -> Result<LlamaCompletionOutput, AdamBackendError> {
    let content = value
        .get("content")
        .or_else(|| value.get("text"))
        .or_else(|| value.get("completion"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(LlamaCompletionOutput {
        content,
        slot_id: value
            .get("slot_id")
            .or_else(|| value.get("id_slot"))
            .and_then(Value::as_i64),
        cache_hit_tokens: value
            .get("cache_hit_tokens")
            .or_else(|| value.pointer("/timings/prompt_cache_hit_tokens"))
            .or_else(|| value.pointer("/timings/cache_n"))
            .or_else(|| value.get("tokens_cached"))
            .and_then(Value::as_i64),
        cache_miss_tokens: value
            .get("cache_miss_tokens")
            .or_else(|| value.pointer("/timings/prompt_cache_miss_tokens"))
            .or_else(|| value.pointer("/timings/prompt_n"))
            .and_then(Value::as_i64),
        provider_response_id: value
            .get("id")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        streamed_text: false,
        raw: value.clone(),
    })
}

pub fn classify_llamacpp_busy(status: StatusCode, value: &Value) -> bool {
    if matches!(
        status,
        StatusCode::CONFLICT | StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
    ) {
        return true;
    }
    provider_error_message(value)
        .map(|message| {
            let normalized = message.to_ascii_lowercase();
            normalized.contains("busy")
                || normalized.contains("slot unavailable")
                || normalized.contains("no idle slot")
                || normalized.contains("all slots are processing")
        })
        .unwrap_or(false)
}

fn provider_error_message(value: &Value) -> Option<String> {
    value
        .get("error")
        .and_then(|error| {
            error.as_str().map(ToString::to_string).or_else(|| {
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
        })
        .or_else(|| {
            value
                .get("message")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
}

/// Parse Adam's strict local JSON tool envelope.
///
/// Non-JSON text is final assistant text. JSON-looking malformed output fails
/// the turn, which prevents accidental natural-language tool execution.
pub fn parse_strict_tool_envelope(
    text: &str,
) -> Result<Option<AdamLocalToolEnvelopeAction>, AdamBackendError> {
    let trimmed = text.trim();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(trimmed).map_err(|err| {
        AdamBackendError::MalformedToolEnvelope(format!("invalid Adam tool envelope JSON: {err}"))
    })?;
    let object = value.as_object().ok_or_else(|| {
        AdamBackendError::MalformedToolEnvelope(
            "Adam tool envelope must be a JSON object".to_string(),
        )
    })?;
    let envelope_type = object.get("type").and_then(Value::as_str).ok_or_else(|| {
        AdamBackendError::MalformedToolEnvelope(
            "Adam tool envelope is missing string field `type`".to_string(),
        )
    })?;
    match envelope_type {
        "adam_final" => {
            let text = object
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    AdamBackendError::MalformedToolEnvelope(
                        "`adam_final` envelope requires string field `text`".to_string(),
                    )
                })?
                .to_string();
            Ok(Some(AdamLocalToolEnvelopeAction::FinalText { text }))
        }
        "adam_tool_call" => {
            let call_id = object
                .get("call_id")
                .or_else(|| object.get("callId"))
                .and_then(Value::as_str)
                .unwrap_or("adam-llama-tool-call")
                .to_string();
            let name = object
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    AdamBackendError::MalformedToolEnvelope(
                        "`adam_tool_call` envelope requires non-empty string field `name`"
                            .to_string(),
                    )
                })?
                .to_string();
            let arguments = object
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            if !arguments.is_object() {
                return Err(AdamBackendError::MalformedToolEnvelope(
                    "`adam_tool_call` field `arguments` must be an object".to_string(),
                ));
            }
            Ok(Some(AdamLocalToolEnvelopeAction::ToolCall {
                call_id,
                name,
                arguments,
            }))
        }
        other => Err(AdamBackendError::MalformedToolEnvelope(format!(
            "unsupported Adam tool envelope type `{other}`"
        ))),
    }
}

pub fn build_responses_request(request: &AdamProviderRequest, model: &str) -> Value {
    let prompt_cache_key = request.prompt_cache_key.clone().unwrap_or_else(|| {
        format!(
            "adam:{}:{}",
            request.turn_context.thread_id, request.session_id
        )
    });
    let mut body = json!({
        "model": model,
        "instructions": request.instructions,
        "input": crate::openai::responses_input_items(&request.input, &request.responses_items),
        "tools": crate::tool_bridge::adam_responses_tool_schemas(&request.tools),
        "tool_choice": if request.tools.is_empty() { "none" } else { "auto" },
        "parallel_tool_calls": false,
        "prompt_cache_key": prompt_cache_key,
        "store": false,
        "stream": false,
        "metadata": {
            "originator": "polar-adam",
            "session_id": request.session_id,
            "thread_id": request.turn_context.thread_id,
            "request_message_id": request.turn_context.request_message_id
        }
    });
    if let Some(max_output_tokens) = request.max_output_tokens {
        body["max_output_tokens"] = json!(max_output_tokens);
    }
    body
}

pub fn parse_responses_response(value: &Value) -> OpenAiResponseOutput {
    let text = value
        .get("output_text")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| collect_responses_text(value));
    let function_calls = collect_responses_function_calls(value);
    OpenAiResponseOutput {
        response_id: value
            .get("id")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        text,
        function_calls,
        usage: value.get("usage").cloned(),
        cache_evidence: value
            .get("usage")
            .and_then(|usage| usage.get("prompt_tokens_details"))
            .cloned(),
        raw: value.clone(),
    }
}

pub fn build_chat_completions_request(request: &AdamProviderRequest, model: &str) -> Value {
    let mut messages = Vec::new();
    if !request.instructions.trim().is_empty() {
        messages.push(json!({
            "role": "system",
            "content": request.instructions
        }));
    }
    messages.push(json!({
        "role": "user",
        "content": request.input
    }));
    if !request.tool_results.is_empty() {
        messages.push(json!({
            "role": "assistant",
            "content": Value::Null,
            "tool_calls": request.tool_results.iter().map(chat_tool_call_from_result).collect::<Vec<_>>()
        }));
    }
    for result in &request.tool_results {
        messages.push(json!({
            "role": "tool",
            "tool_call_id": result.call_id,
            "content": result.output_text
        }));
    }
    let mut body = json!({
        "model": model,
        "messages": messages,
        "tools": request.tools,
        "tool_choice": if request.tools.is_empty() { "none" } else { "auto" },
        "parallel_tool_calls": false,
        "stream": false,
        "metadata": {
            "originator": "polar-adam",
            "session_id": request.session_id,
            "thread_id": request.turn_context.thread_id,
            "request_message_id": request.turn_context.request_message_id,
            "cache_semantics": "openai_compatible_chat_weaker_than_llamacpp_slots"
        }
    });
    if let Some(max_output_tokens) = request.max_output_tokens {
        body["max_tokens"] = json!(max_output_tokens);
    }
    body
}

fn chat_tool_call_from_result(result: &AdamToolResult) -> Value {
    json!({
        "id": result.call_id,
        "type": "function",
        "function": {
            "name": result.name,
            "arguments": serde_json::to_string(&result.arguments).unwrap_or_else(|_| "{}".to_string())
        }
    })
}

pub fn parse_chat_completions_response(value: &Value) -> ChatCompletionOutput {
    let message = value
        .pointer("/choices/0/message")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let text = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let function_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let function = item.get("function")?;
                    let arguments = function
                        .get("arguments")
                        .and_then(Value::as_str)
                        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                        .or_else(|| function.get("arguments").cloned())
                        .unwrap_or_else(|| json!({}));
                    let provider_call_id =
                        item.get("id").and_then(Value::as_str).map(str::to_string);
                    Some(AdamToolCall {
                        call_id: provider_call_id
                            .clone()
                            .unwrap_or_else(|| "chat-tool-call".to_string()),
                        provider_call_id,
                        name: function.get("name")?.as_str()?.to_string(),
                        arguments,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    ChatCompletionOutput {
        response_id: value
            .get("id")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        text,
        function_calls,
        usage: value.get("usage").cloned(),
        raw: value.clone(),
    }
}

fn collect_responses_text(value: &Value) -> String {
    value
        .get("output")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .flat_map(|item| {
                    item.get("content")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                })
                .filter_map(|part| {
                    part.get("text")
                        .or_else(|| part.get("output_text"))
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                })
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn collect_responses_function_calls(value: &Value) -> Vec<AdamToolCall> {
    value
        .get("output")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let item_type = item.get("type").and_then(Value::as_str)?;
                    if item_type != "function_call" {
                        return None;
                    }
                    let arguments = item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                        .or_else(|| item.get("arguments").cloned())
                        .unwrap_or_else(|| json!({}));
                    let provider_call_id = item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    Some(AdamToolCall {
                        call_id: provider_call_id
                            .clone()
                            .unwrap_or_else(|| "responses-tool-call".to_string()),
                        provider_call_id,
                        name: item.get("name")?.as_str()?.to_string(),
                        arguments,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}


