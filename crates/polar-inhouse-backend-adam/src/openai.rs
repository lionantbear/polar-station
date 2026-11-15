use crate::config::OpenAiConfig;
use crate::runner::{
    AdamRunnerProjectionHashes, AdamThinkingSource, AdamToolCall, AdamTurnContext,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;

pub const RESPONSES_WEBSOCKET_BETA_HEADER: &str = "responses_websockets=2026-02-06";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamOAuthCredential {
    pub token_type: String,
    pub access_token: SecretString,
    pub refresh_token: Option<SecretString>,
    pub id_token: Option<SecretString>,
    pub expires_at_ms: Option<i64>,
    pub account_id: Option<String>,
    pub installation_id: Option<String>,
}

impl AdamOAuthCredential {
    pub fn authorization_header(&self) -> String {
        format!("{} {}", self.token_type, self.access_token.expose_secret())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenAiRequest {
    pub request: Value,
    pub headers: BTreeMap<String, String>,
    pub websocket_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAiContinuitySnapshot {
    pub previous_response_id: String,
    pub context_compaction_epoch: i64,
    pub source_digest: String,
    pub renderer_version: String,
    pub model: String,
    pub tool_schema_hash: String,
    pub exact_prefix_hash: String,
    pub input_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenAiPreviousResponseDecision {
    Reuse { previous_response_id: String },
    FullRender { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAiContinuityRequest<'a> {
    pub context_compaction_epoch: i64,
    pub source_digest: &'a str,
    pub renderer_version: &'a str,
    pub model: &'a str,
    pub tool_schema_hash: &'a str,
    pub exact_prefix_hash: &'a str,
    pub input_hash: &'a str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamNormalizedUsage {
    pub input_uncached: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub output: i64,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OpenAiResponsesEvent {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
        source: AdamThinkingSource,
    },
    OutputItemDone {
        item: Value,
    },
    Completed {
        response_id: Option<String>,
        usage: Option<Value>,
        text: Option<String>,
        function_calls: Vec<AdamToolCall>,
    },
    Failed {
        message: String,
    },
    Ignored,
}

pub fn parse_oauth_token_file(
    raw: &str,
    installation_id: Option<String>,
) -> anyhow::Result<AdamOAuthCredential> {
    let value: Value = serde_json::from_str(raw)?;
    let source = value
        .get("tokens")
        .filter(|value| value.is_object())
        .unwrap_or(&value);
    let access_token = string_field(source, &["accessToken", "access_token"])
        .ok_or_else(|| anyhow::anyhow!("Adam OAuth token file is missing access token"))?;
    let refresh_token = string_field(source, &["refreshToken", "refresh_token"]);
    let id_token = string_field(source, &["idToken", "id_token"]);
    let expires_at_ms = integer_field(source, &["expiresAt", "expires_at"]);
    let account_id = string_field(source, &["accountId", "account_id"])
        .or_else(|| extract_account_id(&access_token))
        .or_else(|| id_token.as_deref().and_then(extract_account_id));
    Ok(AdamOAuthCredential {
        token_type: string_field(source, &["tokenType", "token_type"])
            .unwrap_or_else(|| "Bearer".to_string()),
        access_token: SecretString::new(access_token),
        refresh_token: refresh_token.map(SecretString::new),
        id_token: id_token.map(SecretString::new),
        expires_at_ms,
        account_id,
        installation_id,
    })
}

pub fn openai_responses_websocket_url(openai_base_url: &str) -> anyhow::Result<String> {
    let mut url = reqwest::Url::parse(&format!(
        "{}/responses",
        openai_base_url.trim_end_matches('/')
    ))?;
    let protocol = match url.scheme() {
        "http" => "ws",
        "https" => "wss",
        "ws" => "ws",
        "wss" => "wss",
        other => anyhow::bail!("unsupported OpenAI base URL scheme `{other}`"),
    };
    url.set_scheme(protocol)
        .map_err(|_| anyhow::anyhow!("failed to set websocket URL scheme"))?;
    Ok(url.to_string())
}

pub fn build_openai_headers(
    config: &OpenAiConfig,
    credential: &AdamOAuthCredential,
    context: &AdamTurnContext,
    client_request_id: &str,
) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    headers.insert(
        "Authorization".to_string(),
        credential.authorization_header(),
    );
    headers.insert(
        "OpenAI-Beta".to_string(),
        RESPONSES_WEBSOCKET_BETA_HEADER.to_string(),
    );
    headers.insert("User-Agent".to_string(), "polar-adam/0.1".to_string());
    headers.insert("originator".to_string(), config.originator.clone());
    headers.insert(
        "session-id".to_string(),
        provider_safe_id(
            "adam_s",
            &[
                &context.owner_id,
                &context.thread_id,
                &context.inhouse_polar_id,
                &context.backend_profile_id,
            ],
        ),
    );
    headers.insert(
        "thread-id".to_string(),
        provider_safe_id("adam_t", &[&context.owner_id, &context.thread_id]),
    );
    headers.insert(
        "x-client-request-id".to_string(),
        provider_safe_id("adam_r", &[client_request_id]),
    );
    if let Some(account_id) = credential.account_id.as_ref() {
        headers.insert("ChatGPT-Account-Id".to_string(), account_id.clone());
    }
    headers
}

/// Stateless Responses calls replay the user input and the complete tool loop.
pub(crate) fn responses_input_items(input: &str, continuation_items: &[Value]) -> Vec<Value> {
    let mut items = vec![json!({
        "type": "message", "role": "user",
        "content": [{"type": "input_text", "text": input}]
    })];
    items.extend_from_slice(continuation_items);
    items
}

pub fn build_openai_response_create_message(
    config: &OpenAiConfig,
    context: &AdamTurnContext,
    instructions: &str,
    input: &str,
    tools: Vec<Value>,
    tool_outputs: Vec<Value>,
) -> Value {
    build_openai_response_create_message_with_continuity(
        config,
        context,
        instructions,
        input,
        tools,
        tool_outputs,
        None,
    )
}

pub fn build_openai_response_create_message_with_continuity(
    config: &OpenAiConfig,
    context: &AdamTurnContext,
    instructions: &str,
    input: &str,
    tools: Vec<Value>,
    tool_outputs: Vec<Value>,
    previous_response_id: Option<&str>,
) -> Value {
    let input_items = if previous_response_id.is_some() && !tool_outputs.is_empty() {
        tool_outputs
    } else {
        responses_input_items(input, &tool_outputs)
    };
    let mut message = json!({
        "type": "response.create",
        "model": config.model,
        "instructions": instructions,
        "input": input_items,
        "tools": crate::tool_bridge::adam_responses_tool_schemas(&tools),
        "tool_choice": "auto",
        "parallel_tool_calls": config.parallel_tool_calls,
        "reasoning": config.reasoning_effort.as_ref().map(|effort| json!({"effort": effort})),
        "store": false,
        "stream": true,
        "include": ["reasoning.encrypted_content"],
        "text": { "verbosity": "medium" },
        "client_metadata": {
            "originator": config.originator,
            "purpose": "polar-adam",
            "owner_id": context.owner_id,
            "thread_id": context.thread_id,
            "inhouse_polar_id": context.inhouse_polar_id,
            "backend_profile_id": context.backend_profile_id,
            "request_message_id": context.request_message_id
        }
    });
    if let Some(previous_response_id) = previous_response_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        message["previous_response_id"] = json!(previous_response_id);
    }
    message
}

fn provider_safe_id(prefix: &str, parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    let digest = hasher.finalize();
    format!(
        "{}_{}",
        prefix,
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
    )
}

pub fn build_openai_request(
    config: &OpenAiConfig,
    credential: &AdamOAuthCredential,
    context: &AdamTurnContext,
    instructions: &str,
    input: &str,
    tools: Vec<Value>,
) -> anyhow::Result<OpenAiRequest> {
    let client_request_id = format!("{}:{}", context.thread_id, context.request_message_id);
    Ok(OpenAiRequest {
        request: build_openai_response_create_message(
            config,
            context,
            instructions,
            input,
            tools,
            Vec::new(),
        ),
        headers: build_openai_headers(config, credential, context, &client_request_id),
        websocket_url: openai_responses_websocket_url(&config.openai_base_url)?,
    })
}

pub fn decide_openai_previous_response_id(
    snapshot: Option<&OpenAiContinuitySnapshot>,
    request: &OpenAiContinuityRequest<'_>,
) -> OpenAiPreviousResponseDecision {
    let Some(snapshot) = snapshot else {
        return OpenAiPreviousResponseDecision::FullRender {
            reason: "no_previous_response_snapshot".to_string(),
        };
    };
    if snapshot.context_compaction_epoch != request.context_compaction_epoch {
        return OpenAiPreviousResponseDecision::FullRender {
            reason: "context_compaction_epoch_changed".to_string(),
        };
    }
    for (field, previous, current) in [
        (
            "source_digest",
            snapshot.source_digest.as_str(),
            request.source_digest,
        ),
        (
            "renderer_version",
            snapshot.renderer_version.as_str(),
            request.renderer_version,
        ),
        ("model", snapshot.model.as_str(), request.model),
        (
            "tool_schema_hash",
            snapshot.tool_schema_hash.as_str(),
            request.tool_schema_hash,
        ),
        (
            "exact_prefix_hash",
            snapshot.exact_prefix_hash.as_str(),
            request.exact_prefix_hash,
        ),
        (
            "input_hash",
            snapshot.input_hash.as_str(),
            request.input_hash,
        ),
    ] {
        if previous != current {
            return OpenAiPreviousResponseDecision::FullRender {
                reason: format!("{field}_changed"),
            };
        }
    }
    OpenAiPreviousResponseDecision::Reuse {
        previous_response_id: snapshot.previous_response_id.clone(),
    }
}

pub fn openai_continuity_request_from_hashes<'a>(
    context_compaction_epoch: i64,
    source_digest: &'a str,
    model: &'a str,
    hashes: &'a AdamRunnerProjectionHashes,
) -> OpenAiContinuityRequest<'a> {
    OpenAiContinuityRequest {
        context_compaction_epoch,
        source_digest,
        renderer_version: &hashes.renderer_version,
        model,
        tool_schema_hash: &hashes.tool_schema_hash,
        exact_prefix_hash: &hashes.exact_prefix_hash,
        input_hash: &hashes.input_hash,
    }
}

pub fn normalize_openai_usage(usage: Option<&Value>) -> AdamNormalizedUsage {
    let Some(usage) = usage else {
        return AdamNormalizedUsage::default();
    };
    let input = usage_i64(
        usage,
        &[
            "/input_tokens",
            "/prompt_tokens",
            "/usage/input_tokens",
            "/usage/prompt_tokens",
        ],
    );
    let output = usage_i64(
        usage,
        &[
            "/output_tokens",
            "/completion_tokens",
            "/usage/output_tokens",
            "/usage/completion_tokens",
        ],
    );
    let cache_read = usage_i64(
        usage,
        &[
            "/input_tokens_details/cached_tokens",
            "/prompt_tokens_details/cached_tokens",
            "/cache_read_input_tokens",
            "/cached_tokens",
        ],
    );
    let cache_write = usage_i64(
        usage,
        &[
            "/input_tokens_details/cache_write_tokens",
            "/prompt_tokens_details/cache_write_tokens",
            "/input_tokens_details/cache_creation_input_tokens",
            "/prompt_tokens_details/cache_creation_input_tokens",
            "/cache_write_input_tokens",
        ],
    );
    let input_uncached = input.saturating_sub(cache_read).saturating_sub(cache_write);
    let reported_total = usage_i64(usage, &["/total_tokens", "/usage/total_tokens"]);
    AdamNormalizedUsage {
        input_uncached,
        cache_read,
        cache_write,
        output,
        total: reported_total.max(input_uncached + cache_read + cache_write + output),
    }
}

fn usage_i64(value: &Value, pointers: &[&str]) -> i64 {
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

pub fn parse_openai_event(value: &Value) -> OpenAiResponsesEvent {
    match value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "response.output_text.delta" => OpenAiResponsesEvent::TextDelta {
            text: value
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        "response.reasoning_text.delta" | "response.reasoning.text.delta" => {
            OpenAiResponsesEvent::ThinkingDelta {
                text: response_delta_text(value),
                source: AdamThinkingSource::OpenAiReasoningTextDelta,
            }
        }
        "response.reasoning_summary_text.delta"
        | "response.reasoning.summary_text.delta"
        | "response.reasoning_summary.delta"
        | "response.reasoning.summary.delta" => OpenAiResponsesEvent::ThinkingDelta {
            text: response_delta_text(value),
            source: AdamThinkingSource::OpenAiReasoningSummaryTextDelta,
        },
        "response.output_item.done" => OpenAiResponsesEvent::OutputItemDone {
            item: value.get("item").cloned().unwrap_or(Value::Null),
        },
        "response.completed" => {
            let response = value.get("response").unwrap_or(value);
            OpenAiResponsesEvent::Completed {
                response_id: response
                    .get("id")
                    .or_else(|| value.get("id"))
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                usage: response
                    .get("usage")
                    .cloned()
                    .or_else(|| value.get("usage").cloned()),
                text: collect_response_text(response),
                function_calls: collect_function_calls(response),
            }
        }
        "response.failed" | "response.incomplete" | "error" => OpenAiResponsesEvent::Failed {
            message: extract_error_message(value).unwrap_or_else(|| value.to_string()),
        },
        _ => OpenAiResponsesEvent::Ignored,
    }
}

fn collect_response_text(value: &Value) -> Option<String> {
    value
        .get("output_text")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            let text = value
                .get("output")
                .and_then(Value::as_array)?
                .iter()
                .flat_map(|item| {
                    item.get("content")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                })
                .filter_map(|part| {
                    let part_type = part.get("type").and_then(Value::as_str).unwrap_or_default();
                    if part_type != "output_text" && part.get("output_text").is_none() {
                        return None;
                    }
                    part.get("text")
                        .or_else(|| part.get("output_text"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect::<String>();
            (!text.is_empty()).then_some(text)
        })
}

fn response_delta_text(value: &Value) -> String {
    value
        .get("delta")
        .or_else(|| value.get("text"))
        .or_else(|| value.get("summary"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn collect_function_calls(value: &Value) -> Vec<AdamToolCall> {
    value
        .get("output")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    (item.get("type").and_then(Value::as_str) == Some("function_call"))
                        .then_some(())?;
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
                            .unwrap_or_else(|| "openai-tool-call".to_string()),
                        provider_call_id,
                        name: item.get("name")?.as_str()?.to_string(),
                        arguments,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn extract_error_message(value: &Value) -> Option<String> {
    value
        .get("error")
        .and_then(|error| {
            error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
        })
        .or_else(|| {
            value
                .pointer("/response/error/message")
                .and_then(Value::as_str)
        })
        .or_else(|| value.get("message").and_then(Value::as_str))
        .map(ToString::to_string)
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn integer_field(value: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(Value::as_i64).or_else(|| {
            value
                .get(*key)
                .and_then(Value::as_u64)
                .and_then(|raw| i64::try_from(raw).ok())
        })
    })
}

fn extract_account_id(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.as_bytes())
        .ok()?;
    let value: Value = serde_json::from_slice(&decoded).ok()?;
    string_field(
        &value,
        &["https://api.openai.com/auth", "account_id", "accountId"],
    )
}


