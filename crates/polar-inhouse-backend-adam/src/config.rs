use serde::Deserialize;
use serde_json::Value;
use std::fmt;
use std::path::PathBuf;

const DEFAULT_MAX_TOOL_ITERATIONS: usize = 16;
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 300;
const DEFAULT_WEBSOCKET_TIMEOUT_SECS: u64 = 180;
const DEFAULT_OPENAI_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const DEFAULT_AUTH_BASE_URL: &str = "https://auth.openai.com";
const DEFAULT_ORIGINATOR: &str = "polar-adam";
const DEFAULT_OAUTH_VAULT_SCOPE: &str = "inhouse_backend.adam.oauth";
const DEFAULT_OAUTH_VAULT_NAME: &str = "openai-local";
const DEFAULT_LOCAL_OPENAI_BASE_URL: &str = "http://127.0.0.1:1234/v1";
pub const DEFAULT_LLAMA_MAX_OUTPUT_TOKENS: i64 = 20_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdamConfigError {
    InvalidJson(String),
    MissingField(&'static str),
    InvalidField { field: &'static str, reason: String },
    ForbiddenSecretField { path: String },
}

impl fmt::Display for AdamConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(message) => write!(f, "invalid Adam backend config JSON: {message}"),
            Self::MissingField(field) => {
                write!(f, "Adam backend config is missing required field `{field}`")
            }
            Self::InvalidField { field, reason } => {
                write!(f, "invalid Adam backend config field `{field}`: {reason}")
            }
            Self::ForbiddenSecretField { path } => {
                write!(
                    f,
                    "Adam backend config must not contain secret-like field `{path}`"
                )
            }
        }
    }
}

impl std::error::Error for AdamConfigError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AdamLlmConnectionMode {
    OpenAi,
    Llama,
}

impl AdamLlmConnectionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Llama => "llama",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LlamaTransport {
    LlamacppSlots,
    OpenAiCompatibleResponses,
    OpenAiCompatibleChat,
}

impl LlamaTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LlamacppSlots => "llamacpp_slots",
            Self::OpenAiCompatibleResponses => "openai_compatible_responses",
            Self::OpenAiCompatibleChat => "openai_compatible_chat",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SlotStrategy {
    ThreadBound,
}

impl SlotStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ThreadBound => "thread_bound",
        }
    }
}

/// Non-secret Adam backend profile configuration.
///
/// Secrets are intentionally absent. OAuth material is imported through Polar
/// vault and referenced by `oauth_vault_scope`/`oauth_vault_name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdamBackendConfig {
    pub connection: AdamConnectionConfig,
    pub model: String,
    pub max_tool_iterations: usize,
    pub parallel_tool_calls: bool,
    pub request_timeout_secs: u64,
}

/// A selected connection carries its own configuration. It cannot claim one
/// mode while holding another mode's credentials or physical endpoint fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdamConnectionConfig {
    Llama(LlamaConfig),
    OpenAi(OpenAiConfig),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlamaConfig {
    pub transport: LlamaTransport,
    /// Opaque host-owned endpoint reference for native llama.cpp slot control.
    /// Physical URL/model/slot policy is injected only by `AdamBackendRuntime`.
    pub managed_endpoint_id: Option<String>,
    pub base_url: String,
    pub model: String,
    pub slot_strategy: SlotStrategy,
    pub slot_offset: u32,
    pub slot_count: Option<u32>,
    pub cache_prompt: bool,
    /// Provider-dialect control forwarded as the boolean
    /// `chat_template_kwargs.enable_thinking` at the llama.cpp chat-template
    /// boundary. It never rewrites the shared prompt text.
    pub llama_enable_thinking: bool,
    /// Require an initial tool call when tools are available. Once a tool
    /// result exists, the adapter returns to `auto` so the model can answer.
    pub llama_require_tool_call: bool,
    pub prompt_cache_key_strategy: Option<String>,
    pub max_output_tokens: i64,
    pub request_timeout_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiConfig {
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub openai_base_url: String,
    pub auth_base_url: String,
    pub originator: String,
    pub oauth_vault_scope: String,
    pub oauth_vault_name: String,
    pub allow_http_fallback: bool,
    pub max_tool_iterations: usize,
    pub parallel_tool_calls: bool,
    pub websocket_timeout_secs: u64,
    pub request_timeout_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdamAgentConfig {
    pub inhouse_polar_id: String,
    pub backend: AdamBackendConfig,
    pub working_directory: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct RawAdamBackendConfig {
    #[serde(alias = "effort")]
    reasoning_effort: Option<String>,
    llm_connection_mode: Option<String>,
    llama_transport: Option<String>,
    managed_endpoint_id: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
    slot_strategy: Option<String>,
    slot_offset: Option<u32>,
    slot_count: Option<u32>,
    cache_prompt: Option<bool>,
    llama_enable_thinking: Option<bool>,
    llama_require_tool_call: Option<bool>,
    prompt_cache_key_strategy: Option<String>,
    max_output_tokens: Option<i64>,
    openai_base_url: Option<String>,
    auth_base_url: Option<String>,
    originator: Option<String>,
    oauth_vault_scope: Option<String>,
    oauth_vault_name: Option<String>,
    transport: Option<String>,
    allow_http_fallback: Option<bool>,
    max_tool_iterations: Option<usize>,
    parallel_tool_calls: Option<bool>,
    websocket_timeout_secs: Option<u64>,
    request_timeout_secs: Option<u64>,
}

pub fn parse_adam_agent_config(
    inhouse_polar_id: impl Into<String>,
    backend_config_json: &str,
) -> Result<AdamAgentConfig, AdamConfigError> {
    let value: Value = serde_json::from_str(backend_config_json)
        .map_err(|err| AdamConfigError::InvalidJson(err.to_string()))?;
    reject_secret_like_fields(&value)?;
    let raw: RawAdamBackendConfig = serde_json::from_value(value)
        .map_err(|err| AdamConfigError::InvalidJson(err.to_string()))?;
    let backend = AdamBackendConfig::from_raw(raw)?;
    Ok(AdamAgentConfig {
        inhouse_polar_id: inhouse_polar_id.into(),
        backend,
        working_directory: None,
    })
}

impl AdamBackendConfig {
    pub fn connection_mode(&self) -> AdamLlmConnectionMode {
        match self.connection {
            AdamConnectionConfig::Llama(_) => AdamLlmConnectionMode::Llama,
            AdamConnectionConfig::OpenAi(_) => AdamLlmConnectionMode::OpenAi,
        }
    }

    pub fn llama(&self) -> Option<&LlamaConfig> {
        match &self.connection {
            AdamConnectionConfig::Llama(config) => Some(config),
            AdamConnectionConfig::OpenAi(_) => None,
        }
    }

    pub fn llama_mut(&mut self) -> Option<&mut LlamaConfig> {
        match &mut self.connection {
            AdamConnectionConfig::Llama(config) => Some(config),
            AdamConnectionConfig::OpenAi(_) => None,
        }
    }

    pub fn openai(&self) -> Option<&OpenAiConfig> {
        match &self.connection {
            AdamConnectionConfig::OpenAi(config) => Some(config),
            AdamConnectionConfig::Llama(_) => None,
        }
    }

    fn from_raw(raw: RawAdamBackendConfig) -> Result<Self, AdamConfigError> {
        let mode_raw = required_trimmed(raw.llm_connection_mode.as_deref(), "llm_connection_mode")?;
        let mode = parse_connection_mode(mode_raw)?;
        let max_tool_iterations = raw
            .max_tool_iterations
            .unwrap_or(DEFAULT_MAX_TOOL_ITERATIONS);
        if max_tool_iterations == 0 {
            return Err(AdamConfigError::InvalidField {
                field: "max_tool_iterations",
                reason: "must be greater than zero".to_string(),
            });
        }
        let parallel_tool_calls = raw.parallel_tool_calls.unwrap_or(false);
        let request_timeout_secs = raw
            .request_timeout_secs
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS);
        if request_timeout_secs == 0 {
            return Err(AdamConfigError::InvalidField {
                field: "request_timeout_secs",
                reason: "must be greater than zero".to_string(),
            });
        }

        match mode {
            AdamLlmConnectionMode::Llama => {
                let local = LlamaConfig::from_raw(&raw, request_timeout_secs)?;
                let model = local.model.clone();
                Ok(Self {
                    connection: AdamConnectionConfig::Llama(local),
                    model,
                    max_tool_iterations,
                    parallel_tool_calls,
                    request_timeout_secs,
                })
            }
            AdamLlmConnectionMode::OpenAi => {
                let model = required_trimmed(raw.model.as_deref(), "model")?.to_string();
                let openai = OpenAiConfig::from_raw(
                    &raw,
                    &model,
                    max_tool_iterations,
                    parallel_tool_calls,
                    request_timeout_secs,
                )?;
                Ok(Self {
                    connection: AdamConnectionConfig::OpenAi(openai),
                    model,
                    max_tool_iterations,
                    parallel_tool_calls,
                    request_timeout_secs,
                })
            }
        }
    }
}

impl LlamaConfig {
    fn from_raw(
        raw: &RawAdamBackendConfig,
        request_timeout_secs: u64,
    ) -> Result<Self, AdamConfigError> {
        let transport = match raw.llama_transport.as_deref().map(str::trim) {
            None | Some("") => LlamaTransport::LlamacppSlots,
            Some(value) => parse_llama_transport(value)?,
        };
        if transport == LlamaTransport::LlamacppSlots {
            let managed_endpoint_id =
                required_trimmed(raw.managed_endpoint_id.as_deref(), "managed_endpoint_id")?
                    .to_string();
            reject_native_profile_physical_field(raw.base_url.as_ref(), "base_url")?;
            reject_native_profile_physical_field(raw.model.as_ref(), "model")?;
            reject_native_profile_physical_field(raw.slot_strategy.as_ref(), "slot_strategy")?;
            reject_native_profile_physical_field(raw.slot_offset.as_ref(), "slot_offset")?;
            reject_native_profile_physical_field(raw.slot_count.as_ref(), "slot_count")?;
            return Self::new(
                transport,
                Some(managed_endpoint_id),
                String::new(),
                String::new(),
                raw,
                request_timeout_secs,
            );
        }
        if raw
            .managed_endpoint_id
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            return Err(AdamConfigError::InvalidField {
                field: "managed_endpoint_id",
                reason: "is supported only for native `llamacpp_slots` profiles".to_string(),
            });
        }
        let model = required_trimmed(raw.model.as_deref(), "model")?.to_string();
        let base_url = raw
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_LOCAL_OPENAI_BASE_URL)
            .to_string();
        match raw.slot_strategy.as_deref().map(str::trim) {
            None | Some("") | Some("thread_bound") => SlotStrategy::ThreadBound,
            Some(_) => {
                return Err(AdamConfigError::InvalidField {
                    field: "slot_strategy",
                    reason: "only `thread_bound` is supported in Adam v1".to_string(),
                });
            }
        };
        if matches!(raw.slot_count, Some(0)) {
            return Err(AdamConfigError::InvalidField {
                field: "slot_count",
                reason: "must be greater than zero when present".to_string(),
            });
        }
        Self::new(transport, None, base_url, model, raw, request_timeout_secs)
    }

    fn new(
        transport: LlamaTransport,
        managed_endpoint_id: Option<String>,
        base_url: String,
        model: String,
        raw: &RawAdamBackendConfig,
        request_timeout_secs: u64,
    ) -> Result<Self, AdamConfigError> {
        let max_output_tokens = raw
            .max_output_tokens
            .unwrap_or(DEFAULT_LLAMA_MAX_OUTPUT_TOKENS);
        if max_output_tokens <= 0 {
            return Err(AdamConfigError::InvalidField {
                field: "max_output_tokens",
                reason: "must be greater than zero".to_string(),
            });
        }
        Ok(Self {
            transport,
            managed_endpoint_id,
            base_url,
            model,
            slot_strategy: SlotStrategy::ThreadBound,
            slot_offset: raw.slot_offset.unwrap_or(0),
            slot_count: raw.slot_count,
            cache_prompt: raw.cache_prompt.unwrap_or(true),
            llama_enable_thinking: raw.llama_enable_thinking.unwrap_or(true),
            llama_require_tool_call: raw.llama_require_tool_call.unwrap_or(false),
            prompt_cache_key_strategy: raw.prompt_cache_key_strategy.clone(),
            max_output_tokens,
            request_timeout_secs,
        })
    }
}

impl OpenAiConfig {
    fn from_raw(
        raw: &RawAdamBackendConfig,
        model: &str,
        max_tool_iterations: usize,
        parallel_tool_calls: bool,
        request_timeout_secs: u64,
    ) -> Result<Self, AdamConfigError> {
        match raw.transport.as_deref().map(str::trim) {
            None | Some("") | Some("websocket") => {}
            Some(_) => {
                return Err(AdamConfigError::InvalidField {
                    field: "transport",
                    reason: "`openai` only supports `websocket`".to_string(),
                });
            }
        }
        let websocket_timeout_secs = raw
            .websocket_timeout_secs
            .unwrap_or(DEFAULT_WEBSOCKET_TIMEOUT_SECS);
        if websocket_timeout_secs == 0 {
            return Err(AdamConfigError::InvalidField {
                field: "websocket_timeout_secs",
                reason: "must be greater than zero".to_string(),
            });
        }
        Ok(Self {
            model: model.to_string(),
            reasoning_effort: raw.reasoning_effort.as_deref()
                .map(|value| required_trimmed(Some(value), "reasoning_effort").map(str::to_owned))
                .transpose()?,
            openai_base_url: raw
                .openai_base_url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(DEFAULT_OPENAI_BASE_URL)
                .to_string(),
            auth_base_url: raw
                .auth_base_url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(DEFAULT_AUTH_BASE_URL)
                .to_string(),
            originator: raw
                .originator
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(DEFAULT_ORIGINATOR)
                .to_string(),
            oauth_vault_scope: raw
                .oauth_vault_scope
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(DEFAULT_OAUTH_VAULT_SCOPE)
                .to_string(),
            oauth_vault_name: raw
                .oauth_vault_name
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(DEFAULT_OAUTH_VAULT_NAME)
                .to_string(),
            allow_http_fallback: raw.allow_http_fallback.unwrap_or(false),
            max_tool_iterations,
            parallel_tool_calls,
            websocket_timeout_secs,
            request_timeout_secs,
        })
    }
}

fn parse_connection_mode(value: &str) -> Result<AdamLlmConnectionMode, AdamConfigError> {
    match value {
        "openai" => Ok(AdamLlmConnectionMode::OpenAi),
        "llama" => Ok(AdamLlmConnectionMode::Llama),
        _ => Err(AdamConfigError::InvalidField {
            field: "llm_connection_mode",
            reason: format!("unsupported mode `{value}`"),
        }),
    }
}

fn parse_llama_transport(value: &str) -> Result<LlamaTransport, AdamConfigError> {
    match value {
        "llamacpp_slots" => Ok(LlamaTransport::LlamacppSlots),
        "openai_compatible_responses" => Ok(LlamaTransport::OpenAiCompatibleResponses),
        "openai_compatible_chat" => Ok(LlamaTransport::OpenAiCompatibleChat),
        _ => Err(AdamConfigError::InvalidField {
            field: "llama_transport",
            reason: format!("unsupported llama transport `{value}`"),
        }),
    }
}

fn reject_native_profile_physical_field<T>(
    value: Option<&T>,
    field: &'static str,
) -> Result<(), AdamConfigError> {
    if value.is_some() {
        return Err(AdamConfigError::InvalidField {
            field,
            reason: "native llamacpp_slots endpoint policy is host-owned; remove this field"
                .to_string(),
        });
    }
    Ok(())
}

fn required_trimmed<'a>(
    value: Option<&'a str>,
    field: &'static str,
) -> Result<&'a str, AdamConfigError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(AdamConfigError::MissingField(field))
}

fn reject_secret_like_fields(value: &Value) -> Result<(), AdamConfigError> {
    reject_secret_like_fields_at(value, "$")
}

fn reject_secret_like_fields_at(value: &Value, path: &str) -> Result<(), AdamConfigError> {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let child_path = format!("{path}.{key}");
                if is_secret_like_key(key) {
                    return Err(AdamConfigError::ForbiddenSecretField { path: child_path });
                }
                reject_secret_like_fields_at(child, &child_path)?;
            }
        }
        Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                reject_secret_like_fields_at(child, &format!("{path}[{index}]"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn is_secret_like_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    normalized.contains("apikey")
        || normalized.contains("accesstoken")
        || normalized.contains("refreshtoken")
        || normalized.contains("bearertoken")
        || normalized.contains("idtoken")
        || normalized.contains("authorization")
        || normalized.contains("password")
        || normalized.contains("secret")
        || normalized == "token"
        || normalized.ends_with("tokenenv")
}


