//! Restricted Host-to-Host platform notification contract.
use crate::{validate_stable_identifier, ProtocolValidationError};
use base64::Engine;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const NOTIFICATION_API_VERSION: u16 = 1;
pub const IMESSAGE_MAX_SCALARS: usize = 1200;
pub const NOTIFICATION_MAX_ENVELOPE_BYTES: usize = 16 * 1024;

/// One source may submit notifications; this never grants execution/root authority.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
pub struct NotificationPairingV1 {
    pub schema: String,
    pub source_host_id: String,
    pub sender_host_id: String,
    pub sender_generation: String,
    pub destination_ref: String,
    pub notification_key: String,
}
impl std::fmt::Debug for NotificationPairingV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotificationPairingV1")
            .field("notification_key", &"<redacted>")
            .finish_non_exhaustive()
    }
}
impl NotificationPairingV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.schema != "polar-station-notification-pair-v1" {
            return Err(ProtocolValidationError::InvalidIdentifier(
                "notification pairing schema",
            ));
        }
        for value in [
            &self.source_host_id,
            &self.sender_host_id,
            &self.sender_generation,
            &self.destination_ref,
        ] {
            validate_stable_identifier(value, "notification pairing identity")?;
        }
        let key = zeroize::Zeroizing::new(
            base64::engine::general_purpose::STANDARD
                .decode(&self.notification_key)
                .map_err(|_| {
                    ProtocolValidationError::InvalidIdentifier("notification pairing key")
                })?,
        );
        if key.len() != 32 {
            return Err(ProtocolValidationError::InvalidIdentifier(
                "notification pairing key",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NotificationCommandV1 {
    Connector {
        command: Box<crate::ConnectorCommandV1>,
    },
    SetupCurrentSender {
        expected_revision: u64,
        source_host_id: String,
    },
    GetSenderStatus,
    GetSenderSettings,
    SetSenderRecipient {
        expected_revision: u64,
        recipient: String,
    },
    UpdateSenderSettings {
        expected_revision: u64,
        settings: polar_station_settings::ImessageSenderSettingsV1,
    },
    SendNotification {
        request_id: String,
        expected_sender_generation: String,
        destination_ref: String,
        text: String,
    },
    GetReceipt {
        request_id: String,
        sender_generation: String,
    },
    RequestMessagesAuthorization,
}
impl NotificationCommandV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::Connector { command } => command.validate(),
            Self::SetupCurrentSender { source_host_id, .. } => {
                validate_stable_identifier(source_host_id, "source Host")
            }
            Self::GetSenderStatus
            | Self::GetSenderSettings
            | Self::RequestMessagesAuthorization => Ok(()),
            Self::SetSenderRecipient { recipient, .. } => {
                if recipient.is_empty()
                    || recipient.len() > 512
                    || recipient.chars().any(char::is_control)
                {
                    return Err(ProtocolValidationError::InvalidIdentifier(
                        "Messages recipient",
                    ));
                }
                Ok(())
            }
            Self::UpdateSenderSettings { settings, .. } => {
                let mut proposed = settings.clone();
                proposed.sender_generation = "assigned-by-host".into();
                proposed.validate().map_err(|_| {
                    ProtocolValidationError::InvalidIdentifier("notification settings")
                })
            }
            Self::GetReceipt {
                request_id,
                sender_generation,
            } => {
                validate_stable_identifier(request_id, "notification request")?;
                validate_stable_identifier(sender_generation, "notification generation")
            }
            Self::SendNotification {
                request_id,
                expected_sender_generation,
                destination_ref,
                text,
            } => {
                validate_stable_identifier(request_id, "notification request")?;
                validate_stable_identifier(expected_sender_generation, "notification generation")?;
                validate_stable_identifier(destination_ref, "notification destination")?;
                crate::validate_codex_input(text)?;
                if text.chars().count() > IMESSAGE_MAX_SCALARS {
                    return Err(ProtocolValidationError::InvalidIdentifier(
                        "notification length",
                    ));
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationReceiptStatusV1 {
    Sending,
    SentToMessages,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationReceiptV1 {
    pub request_id: String,
    pub sender_generation: String,
    pub status: NotificationReceiptStatusV1,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationSenderStatusV1 {
    pub host_id: String,
    pub supported: bool,
    pub enabled: bool,
    pub authorized: bool,
    pub sender_generation: String,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NotificationResultV1 {
    Connector {
        result: Box<crate::ConnectorResultV1>,
    },
    Pairing {
        pairing: NotificationPairingV1,
    },
    Settings {
        settings: polar_station_settings::ImessageSenderSettingsV1,
    },
    Status {
        status: NotificationSenderStatusV1,
    },
    Receipt {
        receipt: NotificationReceiptV1,
    },
    Error {
        code: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationApiV1 {
    NotificationV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationRequestEnvelopeV1 {
    pub protocol_version: u32,
    pub api: NotificationApiV1,
    pub request_id: String,
    pub command: NotificationCommandV1,
}
impl NotificationRequestEnvelopeV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.protocol_version != crate::PROTOCOL_VERSION {
            return Err(ProtocolValidationError::InvalidIdentifier(
                "notification protocol",
            ));
        }
        validate_stable_identifier(&self.request_id, "notification request")?;
        self.command.validate()?;
        validate_envelope_size(
            self,
            matches!(self.command, NotificationCommandV1::Connector { .. }),
        )
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationResponseEnvelopeV1 {
    pub protocol_version: u32,
    pub api: NotificationApiV1,
    pub request_id: String,
    pub result: NotificationResultV1,
}
impl NotificationResponseEnvelopeV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.protocol_version != crate::PROTOCOL_VERSION {
            return Err(ProtocolValidationError::InvalidIdentifier(
                "notification protocol",
            ));
        }
        validate_stable_identifier(&self.request_id, "notification request")?;
        validate_envelope_size(
            self,
            matches!(self.result, NotificationResultV1::Connector { .. }),
        )
    }
}

fn validate_envelope_size(
    value: &impl Serialize,
    connector: bool,
) -> Result<(), ProtocolValidationError> {
    let bytes = serde_json::to_vec(value).map_err(|_| {
        ProtocolValidationError::InvalidIdentifier("notification envelope encoding")
    })?;
    if bytes.len()
        > if connector {
            256 * 1024
        } else {
            NOTIFICATION_MAX_ENVELOPE_BYTES
        }
    {
        return Err(ProtocolValidationError::InvalidIdentifier(
            "notification envelope length",
        ));
    }
    Ok(())
}
