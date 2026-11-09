//! Canonical reusable configuration. Legacy backend JSON is only an adapter shape.
use crate::InhouseBackendProfileRecord;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub type Settings = Map<String, Value>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Harness {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub driver: String,
    #[serde(default)]
    pub settings: Settings,
}

/// A connection dialect is distinct from the service/vendor label. Native
/// dialects retain the selected harness's authentication and provider semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionKind {
    OpenaiOauth,
    OpenaiResponses,
    OpenaiChat,
    Llamacpp,
    CodexNative,
    PiNative,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub kind: ConnectionKind,
    /// Explicit connection parameters, including credential *references*.
    /// Native dialect fields retain their native names for lossless conversion.
    #[serde(default)]
    pub settings: Settings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub harness_id: String,
    /// None only for a harness which owns its external connection (ACP/Web).
    pub provider_id: Option<String>,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Configuration {
    pub revision: u64,
    pub harnesses: Vec<Harness>,
    pub providers: Vec<Provider>,
    pub profiles: Vec<Profile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Change {
    Harness(Harness),
    Provider(Provider),
    Profile(Profile),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Write {
    pub expected_revision: u64,
    pub change: Change,
}

const PROFILE_FIELDS: &[&str] = &[
    "model",
    "effort",
    "reasoning_effort",
    "pi_thinking",
    "cwd",
    "conversation_model_metadata",
    "max_output_tokens",
    "temperature",
    "top_p",
    "memory_extract_model",
    "memory_consolidation_model",
];
const ADAM_CONNECTION_FIELDS: &[&str] = &[
    "llm_connection_mode",
    "llama_transport",
    "managed_endpoint_id",
    "base_url",
    "openai_base_url",
    "auth_base_url",
    "originator",
    "oauth_vault_scope",
    "oauth_vault_name",
    "transport",
    "allow_http_fallback",
];
const CODEX_CONNECTION_FIELDS: &[&str] = &[
    "auth_home",
    "model_provider",
    "provider_base_url",
    "oss_mode",
    "lmstudio_host",
];
const PI_CONNECTION_FIELDS: &[&str] = &[
    "pi_provider",
    "pi_base_url",
    "pi_api_key",
    "pi_api_key_vault_scope",
    "pi_api_key_vault_name",
];

fn connection_fields(driver: &str) -> &'static [&'static str] {
    match driver {
        "adam" => ADAM_CONNECTION_FIELDS,
        "codex" => CODEX_CONNECTION_FIELDS,
        "pi" => PI_CONNECTION_FIELDS,
        _ => &[],
    }
}

fn take_fields(settings: &mut Settings, keys: &[&str]) -> Settings {
    keys.iter()
        .filter_map(|key| settings.remove(*key).map(|value| ((*key).into(), value)))
        .collect()
}

/// Deterministic private components preserve old accounts instead of guessing
/// equality from a URL, account name or provider label.
pub fn split(
    record: &InhouseBackendProfileRecord,
) -> Result<(Harness, Option<Provider>, Profile), String> {
    let mut settings: Settings =
        serde_json::from_str(&record.backend_config_json).map_err(|e| e.to_string())?;
    let mut profile_settings = take_fields(&mut settings, PROFILE_FIELDS);
    let effort_key = match record.backend_driver.as_str() {
        "codex" => "effort",
        "pi" => "pi_thinking",
        _ => "reasoning_effort",
    };
    if effort_key != "reasoning_effort" {
        if let Some(value) = profile_settings.remove(effort_key) {
            profile_settings.insert("reasoning_effort".into(), value);
        }
    }
    let connection_settings = take_fields(&mut settings, connection_fields(&record.backend_driver));
    let kind = match record.backend_driver.as_str() {
        "adam" => Some(
            match connection_settings
                .get("llm_connection_mode")
                .and_then(Value::as_str)
            {
                Some("openai") => ConnectionKind::OpenaiOauth,
                _ => match connection_settings
                    .get("llama_transport")
                    .and_then(Value::as_str)
                {
                    Some("openai_compatible_responses") => ConnectionKind::OpenaiResponses,
                    Some("openai_compatible_chat") => ConnectionKind::OpenaiChat,
                    _ => ConnectionKind::Llamacpp,
                },
            },
        ),
        "codex" => Some(ConnectionKind::CodexNative),
        "pi" => Some(ConnectionKind::PiNative),
        _ => None,
    };
    let harness_id = format!("legacy-harness:{}", record.backend_profile_id);
    let provider = kind.map(|kind| Provider {
        id: format!("legacy-provider:{}", record.backend_profile_id),
        name: format!("{} connection", record.display_name),
        enabled: true,
        kind,
        settings: connection_settings,
    });
    let profile = Profile {
        id: record.backend_profile_id.clone(),
        name: record.display_name.clone(),
        enabled: record.enabled,
        harness_id: harness_id.clone(),
        provider_id: provider.as_ref().map(|p| p.id.clone()),
        settings: profile_settings,
        created_at: record.created_at.as_secs(),
        updated_at: record.updated_at.as_secs(),
    };
    Ok((
        Harness {
            id: harness_id,
            name: format!("{} harness", record.display_name),
            enabled: true,
            driver: record.backend_driver.clone(),
            settings,
        },
        provider,
        profile,
    ))
}

impl Provider {
    pub fn validate(&self) -> Result<(), String> {
        let driver = match self.kind {
            ConnectionKind::CodexNative => "codex",
            ConnectionKind::PiNative => "pi",
            _ => "adam",
        };
        if self
            .settings
            .keys()
            .any(|key| !connection_fields(driver).contains(&key.as_str()))
        {
            return Err("Provider contains settings outside its connection dialect".into());
        }
        if driver == "adam" {
            let mode = self
                .settings
                .get("llm_connection_mode")
                .and_then(Value::as_str)
                .unwrap_or("llama");
            let transport = self
                .settings
                .get("llama_transport")
                .and_then(Value::as_str)
                .unwrap_or("llamacpp_slots");
            let matches = match self.kind {
                ConnectionKind::OpenaiOauth => mode == "openai",
                ConnectionKind::OpenaiResponses => {
                    mode == "llama" && transport == "openai_compatible_responses"
                }
                ConnectionKind::OpenaiChat => {
                    mode == "llama" && transport == "openai_compatible_chat"
                }
                ConnectionKind::Llamacpp => mode == "llama" && transport == "llamacpp_slots",
                _ => false,
            };
            if !matches {
                return Err("Provider kind conflicts with its connection settings".into());
            }
        }
        for (scope, name) in [
            ("pi_api_key_vault_scope", "pi_api_key_vault_name"),
            ("oauth_vault_scope", "oauth_vault_name"),
        ] {
            if self.settings.contains_key(scope) != self.settings.contains_key(name)
                && scope.starts_with("pi_")
            {
                return Err("Credential reference requires both scope and name".into());
            }
        }
        Ok(())
    }
    pub fn supports(&self, driver: &str) -> bool {
        matches!(
            (self.kind, driver),
            (
                ConnectionKind::OpenaiOauth
                    | ConnectionKind::OpenaiResponses
                    | ConnectionKind::OpenaiChat
                    | ConnectionKind::Llamacpp,
                "adam"
            ) | (ConnectionKind::CodexNative, "codex")
                | (ConnectionKind::PiNative, "pi")
        )
    }
}

pub fn compose(
    profile: &Profile,
    harness: &Harness,
    provider: Option<&Provider>,
) -> Result<InhouseBackendProfileRecord, String> {
    if profile.harness_id != harness.id {
        return Err("Profile references a different Harness".into());
    }
    let mut settings = harness.settings.clone();
    if let Some(provider) = provider {
        provider.validate()?;
        if profile.provider_id.as_deref() != Some(provider.id.as_str())
            || !provider.supports(&harness.driver)
        {
            return Err("Provider connection is incompatible with the selected Harness".into());
        }
        for (key, value) in &provider.settings {
            if !connection_fields(&harness.driver).contains(&key.as_str()) {
                return Err(format!(
                    "{key} is not a connection setting for {}",
                    harness.driver
                ));
            }
            if settings.insert(key.clone(), value.clone()).is_some() {
                return Err(format!("Duplicate configuration field: {key}"));
            }
        }
    } else if profile.provider_id.is_some()
        || matches!(harness.driver.as_str(), "adam" | "codex" | "pi")
    {
        return Err("Selected Harness requires an explicit Provider".into());
    }
    for (key, value) in &profile.settings {
        if !PROFILE_FIELDS.contains(&key.as_str()) {
            return Err(format!("{key} is not a Profile setting"));
        }
        let key = if key == "reasoning_effort" {
            match harness.driver.as_str() {
                "codex" => "effort",
                "pi" => "pi_thinking",
                _ => key,
            }
        } else {
            key
        };
        if settings.insert(key.to_owned(), value.clone()).is_some() {
            return Err(format!("Duplicate configuration field: {key}"));
        }
    }
    Ok(InhouseBackendProfileRecord {
        backend_profile_id: profile.id.clone(),
        display_name: profile.name.clone(),
        enabled: profile.enabled && harness.enabled && provider.is_none_or(|p| p.enabled),
        backend_driver: harness.driver.clone(),
        backend_config_json: Value::Object(settings).to_string(),
        created_at: polar_identity::UnixTimestamp::from_secs(profile.created_at),
        updated_at: polar_identity::UnixTimestamp::from_secs(profile.updated_at),
    })
}

impl Configuration {
    pub fn resolve(&self, id: &str) -> Result<InhouseBackendProfileRecord, String> {
        let profile = self
            .profiles
            .iter()
            .find(|p| p.id == id)
            .ok_or("Profile not found")?;
        let harness = self
            .harnesses
            .iter()
            .find(|h| h.id == profile.harness_id)
            .ok_or("Harness not found")?;
        let provider = profile
            .provider_id
            .as_ref()
            .map(|id| {
                self.providers
                    .iter()
                    .find(|p| &p.id == id)
                    .ok_or("Provider not found")
            })
            .transpose()?;
        compose(profile, harness, provider)
    }

    pub fn apply(&mut self, change: Change) -> Result<(), String> {
        let mut next = self.clone();
        next.apply_inner(change)?;
        *self = next;
        Ok(())
    }

    fn apply_inner(&mut self, change: Change) -> Result<(), String> {
        fn put<T>(rows: &mut Vec<T>, value: T, id: impl Fn(&T) -> &str) {
            if let Some(index) = rows.iter().position(|v| id(v) == id(&value)) {
                rows[index] = value;
            } else {
                rows.push(value);
            }
        }
        match change {
            Change::Harness(value) => {
                identifier(&value.id)?;
                required(&value.name)?;
                required(&value.driver)?;
                if value.settings.keys().any(|key| {
                    PROFILE_FIELDS.contains(&key.as_str())
                        || connection_fields(&value.driver).contains(&key.as_str())
                }) {
                    return Err(
                        "Harness cannot own model, connection or working-directory settings".into(),
                    );
                }
                put(&mut self.harnesses, value, |v| &v.id);
            }
            Change::Provider(value) => {
                identifier(&value.id)?;
                required(&value.name)?;
                value.validate()?;
                put(&mut self.providers, value, |v| &v.id);
            }
            Change::Profile(value) => {
                identifier(&value.id)?;
                required(&value.name)?;
                identifier(&value.harness_id)?;
                if let Some(id) = &value.provider_id {
                    identifier(id)?;
                }
                if value.created_at > i64::MAX as u64 || value.updated_at > i64::MAX as u64 {
                    return Err("Profile timestamp is out of range".into());
                }
                put(&mut self.profiles, value, |v| &v.id);
            }
        }
        for profile in &self.profiles {
            self.resolve(&profile.id)?;
        }
        Ok(())
    }
}

fn required(value: &str) -> Result<(), String> {
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err("Configuration identifiers/names must be nonempty, trimmed and contain no control characters".into());
    }
    Ok(())
}

fn identifier(value: &str) -> Result<(), String> {
    required(value)?;
    if value.contains(['/', '\\']) || value == "." || value == ".." {
        return Err("Configuration ID cannot contain path separators".into());
    }
    Ok(())
}
