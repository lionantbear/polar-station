//! Original event identity and provider stream routing, independent of observers.
use crate::{validate_codex_input, validate_stable_identifier, ProtocolValidationError};
use polar_station_settings::MessageConnectorSettingsV1;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorPublicationV1 {
    pub connector_id: String,
    pub destination_ref: String,
    pub endpoint_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub event_kind: String,
    pub thread_name: String,
    pub text: String,
}
impl ConnectorPublicationV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        for id in [
            &self.connector_id,
            &self.destination_ref,
            &self.endpoint_id,
            &self.thread_id,
            &self.turn_id,
        ] {
            validate_stable_identifier(id, "connector publication")?;
        }
        if !matches!(
            self.event_kind.as_str(),
            "completed" | "failed" | "interrupted"
        ) || self.thread_name.len() > 512
        {
            return Err(ProtocolValidationError::InvalidIdentifier(
                "connector completion",
            ));
        }
        validate_codex_input(&self.text)
    }
    pub fn stream_id(&self) -> String {
        stable_connector_id(
            "stream",
            &[&self.connector_id, &self.destination_ref, &self.thread_id],
        )
    }
    pub fn delivery_id(&self) -> String {
        stable_connector_id(
            "delivery",
            &[&self.stream_id(), &self.turn_id, &self.event_kind],
        )
    }
}
pub fn stable_connector_id(kind: &str, parts: &[&str]) -> String {
    format!(
        "{kind}-{:x}",
        Sha256::digest(serde_json::to_vec(parts).expect("string tuple"))
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConnectorCommandV1 {
    GetSettings,
    GetStreams {
        connector_id: String,
    },
    SetReturnRoute {
        stream_id: String,
        expected_host_id: String,
        host_id: String,
        endpoint_id: String,
    },
    UpdateSettings {
        expected_revision: u64,
        settings: MessageConnectorSettingsV1,
    },
    Publish {
        publication: ConnectorPublicationV1,
    },
    GetReceipt {
        connector_id: String,
        delivery_id: String,
    },
    Subscribe {
        connector_id: String,
    },
    AcknowledgeInbound {
        connector_id: String,
        inbound_id: String,
        status: String,
    },
}
impl ConnectorCommandV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::GetSettings => Ok(()),
            Self::GetStreams { connector_id } => {
                validate_stable_identifier(connector_id, "connector")
            }
            Self::SetReturnRoute {
                stream_id,
                expected_host_id,
                host_id,
                endpoint_id,
            } => {
                for id in [stream_id, expected_host_id, host_id, endpoint_id] {
                    validate_stable_identifier(id, "return route")?;
                }
                Ok(())
            }
            Self::UpdateSettings { settings, .. } => settings
                .validate()
                .map_err(|_| ProtocolValidationError::InvalidIdentifier("connector settings")),
            Self::Publish { publication } => publication.validate(),
            Self::Subscribe { connector_id } => {
                validate_stable_identifier(connector_id, "connector")
            }
            Self::GetReceipt {
                connector_id,
                delivery_id,
            } => {
                validate_stable_identifier(connector_id, "connector")?;
                validate_stable_identifier(delivery_id, "delivery")
            }
            Self::AcknowledgeInbound {
                connector_id,
                inbound_id,
                status,
            } => {
                validate_stable_identifier(connector_id, "connector")?;
                validate_stable_identifier(inbound_id, "inbound")?;
                if !matches!(status.as_str(), "accepted" | "rejected" | "unknown") {
                    return Err(ProtocolValidationError::InvalidIdentifier("inbound result"));
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorInboundV1 {
    pub id: String,
    pub connector_id: String,
    pub stream_id: String,
    pub endpoint_id: String,
    pub thread_id: String,
    pub text: String,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectorResultV1 {
    Streams {
        streams: Vec<ConnectorStreamV1>,
    },
    Settings {
        host_id: String,
        settings: MessageConnectorSettingsV1,
    },
    Receipt {
        delivery_id: String,
        stream_id: String,
        status: String,
        provider_message_id: Option<String>,
        error_code: Option<String>,
    },
    Inbound {
        messages: Vec<ConnectorInboundV1>,
    },
    Acknowledged,
    Error {
        code: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorStreamV1 {
    pub id: String,
    pub connector_id: String,
    pub destination_ref: String,
    pub thread_id: String,
    pub title: String,
    pub return_host_id: String,
    pub return_endpoint_id: String,
    pub status: String,
    pub platform_id: Option<String>,
    pub error_code: Option<String>,
}
