//! Revisioned settings for original-session control and the embedded Mac sender.
use crate::ValidationError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexControlModeV1 {
    #[default]
    Local,
    RemoteHost,
    SshDirect,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexEndpointSettingsV1 {
    pub id: String,
    pub execution_target_id: String,
    pub control_mode: CodexControlModeV1,
    pub owner_host_id: String,
    /// Path interpreted on the execution machine, never a temporary SSH forward.
    pub socket_path: String,
    pub peer_connection_ref: Option<String>,
    #[serde(default)]
    pub remote_endpoint_id: Option<String>,
    #[serde(default)]
    pub remote_target_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramAllowedSourceV1 {
    pub bot_id: i64,
    pub chat_id: i64,
    pub sender_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramDestinationV1 {
    pub id: String,
    pub bot_id: i64,
    pub chat_id: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramControlSettingsV1 {
    pub enabled: bool,
    pub bot_credential_ref: Option<String>,
    pub allowed_sources: Vec<TelegramAllowedSourceV1>,
    pub destinations: Vec<TelegramDestinationV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImessageDestinationV1 {
    pub host_id: String,
    pub connection_ref: Option<String>,
    pub sender_generation: String,
    pub destination_ref: String,
}

/// Routing metadata only; each direction/scope has its own secure-store entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostConnectionSettingsV1 {
    pub id: String,
    pub host_id: String,
    pub ssh_target_id: String,
    pub port: u16,
    pub execution_credential_ref: Option<String>,
    pub notification_credential_ref: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexControlSettingsV1 {
    pub revision: u64,
    pub enabled: bool,
    pub endpoints: Vec<CodexEndpointSettingsV1>,
    #[serde(default)]
    pub host_connections: Vec<HostConnectionSettingsV1>,
    pub telegram: TelegramControlSettingsV1,
    pub imessage_enabled: bool,
    pub imessage_sender: Option<ImessageDestinationV1>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImessageSenderSettingsV1 {
    pub revision: u64,
    pub enabled: bool,
    pub recipient_credential_ref: Option<String>,
    pub sender_generation: String,
    pub allowed_source_principals: Vec<String>,
}

fn text(value: &str, field: &'static str) -> Result<(), ValidationError> {
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err(ValidationError::InvalidText(field));
    }
    Ok(())
}

impl CodexControlSettingsV1 {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.endpoints.len() > 64
            || self.host_connections.len() > 64
            || self.telegram.allowed_sources.len() > 64
            || self.telegram.destinations.len() > 64
        {
            return Err(ValidationError::InvalidText("Codex settings limit"));
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut execution_owners = std::collections::BTreeSet::new();
        for connection in &self.host_connections {
            text(&connection.id, "Host connection")?;
            text(&connection.host_id, "peer Host")?;
            text(&connection.ssh_target_id, "peer SSH target")?;
            if connection.port == 0 || !ids.insert(&connection.id) {
                return Err(ValidationError::InvalidText("Host connection identity"));
            }
            if connection.execution_credential_ref.is_some()
                && !execution_owners.insert(&connection.host_id)
            {
                return Err(ValidationError::InvalidText(
                    "ambiguous execution Host connection",
                ));
            }
            for reference in [
                &connection.execution_credential_ref,
                &connection.notification_credential_ref,
            ]
            .into_iter()
            .flatten()
            {
                text(reference, "peer credential reference")?;
            }
        }
        ids.clear();
        let mut remote_routes = std::collections::BTreeSet::new();
        for endpoint in &self.endpoints {
            text(&endpoint.id, "Codex endpoint")?;
            text(&endpoint.owner_host_id, "Codex owner")?;
            text(&endpoint.execution_target_id, "Codex target")?;
            if !ids.insert(&endpoint.id) {
                return Err(ValidationError::InvalidText("duplicate Codex endpoint"));
            }
            match endpoint.control_mode {
                CodexControlModeV1::RemoteHost => {
                    text(
                        endpoint.remote_endpoint_id.as_deref().unwrap_or_default(),
                        "remote Codex endpoint",
                    )?;
                    text(
                        endpoint.remote_target_id.as_deref().unwrap_or_default(),
                        "remote execution target",
                    )?;
                    if !remote_routes.insert((
                        &endpoint.owner_host_id,
                        &endpoint.remote_endpoint_id,
                        &endpoint.remote_target_id,
                    )) {
                        return Err(ValidationError::InvalidText(
                            "ambiguous remote Codex mapping",
                        ));
                    }
                    if !self.host_connections.iter().any(|c| {
                        Some(&c.id) == endpoint.peer_connection_ref.as_ref()
                            && c.host_id == endpoint.owner_host_id
                            && c.execution_credential_ref.is_some()
                    }) {
                        return Err(ValidationError::InvalidText("execution Host connection"));
                    }
                }
                CodexControlModeV1::Local | CodexControlModeV1::SshDirect => {
                    text(&endpoint.socket_path, "Codex socket")?;
                    if !endpoint.socket_path.starts_with('/') {
                        return Err(ValidationError::InvalidText("absolute Codex socket"));
                    }
                }
            }
        }
        if self.telegram.enabled {
            text(
                self.telegram
                    .bot_credential_ref
                    .as_deref()
                    .unwrap_or_default(),
                "Telegram credential reference",
            )?;
            if self.telegram.allowed_sources.is_empty() {
                return Err(ValidationError::InvalidText("Telegram allowed sources"));
            }
        }
        for source in &self.telegram.allowed_sources {
            if source.bot_id <= 0 || source.chat_id == 0 || source.sender_id <= 0 {
                return Err(ValidationError::InvalidText("Telegram source identity"));
            }
        }
        ids.clear();
        for destination in &self.telegram.destinations {
            text(&destination.id, "Telegram destination")?;
            if destination.bot_id <= 0 || destination.chat_id == 0 || !ids.insert(&destination.id) {
                return Err(ValidationError::InvalidText(
                    "Telegram destination identity",
                ));
            }
        }
        if self.imessage_enabled && self.imessage_sender.is_none() {
            return Err(ValidationError::InvalidText("iMessage sender"));
        }
        if let Some(sender) = &self.imessage_sender {
            text(&sender.host_id, "iMessage Host")?;
            text(&sender.sender_generation, "iMessage generation")?;
            text(&sender.destination_ref, "iMessage destination")?;
            if let Some(reference) = &sender.connection_ref {
                if !self.host_connections.iter().any(|c| {
                    &c.id == reference
                        && c.host_id == sender.host_id
                        && c.notification_credential_ref.is_some()
                }) {
                    return Err(ValidationError::InvalidText("notification Host connection"));
                }
            }
        }
        Ok(())
    }
}

impl ImessageSenderSettingsV1 {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.allowed_source_principals.len() > 64 {
            return Err(ValidationError::InvalidText("iMessage source limit"));
        }
        for principal in &self.allowed_source_principals {
            text(principal, "iMessage source")?;
        }
        if self.enabled {
            text(
                self.recipient_credential_ref.as_deref().unwrap_or_default(),
                "iMessage recipient reference",
            )?;
            text(&self.sender_generation, "iMessage generation")?;
        }
        Ok(())
    }
}
