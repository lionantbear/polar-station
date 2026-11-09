use polar_station_settings::{
    AppearanceSettingsV1, AttentionSettingsV1, MonitoringSettingsV1, SessionSummarySettingsV1,
    SettingsV1,
};
use serde::{Deserialize, Serialize};

use crate::{validate_stable_identifier, ProtocolError, ProtocolValidationError};

pub const SETTINGS_API_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsApiV1 {
    SettingsV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexNotifyConfigStateV1 {
    Missing,
    Configured,
    Conflict,
    Invalid,
    Unsafe,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SettingsCommandV1 {
    GetProjection,
    UpdateAppearance { value: AppearanceSettingsV1 },
    UpdateAttention { value: AttentionSettingsV1 },
    UpdateMonitoring { value: MonitoringSettingsV1 },
    UpdateSessionSummary { value: SessionSummarySettingsV1 },
    TestSessionSummaryProvider { value: SessionSummarySettingsV1 },
    GetCodexNotifyStatus { execution_target_id: String },
    ConfigureCodexNotify { execution_target_id: String },
}

impl SettingsCommandV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if let Self::GetCodexNotifyStatus {
            execution_target_id,
        }
        | Self::ConfigureCodexNotify {
            execution_target_id,
        } = self
        {
            return validate_stable_identifier(execution_target_id, "Codex setup target");
        }
        let result = match self {
            Self::GetProjection => Ok(()),
            Self::UpdateAppearance { value } => value.validate(),
            Self::UpdateAttention { .. } => Ok(()),
            Self::UpdateMonitoring { value } => value.validate(),
            Self::UpdateSessionSummary { value } | Self::TestSessionSummaryProvider { value } => {
                value.validate()
            }
            Self::GetCodexNotifyStatus { .. } | Self::ConfigureCodexNotify { .. } => unreachable!(),
        };
        result.map_err(|_| ProtocolValidationError::InvalidIdentifier("Settings value"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsRequestEnvelopeV1 {
    pub protocol_version: u32,
    pub api: SettingsApiV1,
    pub request_id: String,
    pub expected_revision: Option<u64>,
    pub command: SettingsCommandV1,
}

impl SettingsRequestEnvelopeV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.protocol_version != crate::PROTOCOL_VERSION || self.api != SettingsApiV1::SettingsV1
        {
            return Err(ProtocolValidationError::InvalidIdentifier(
                "Settings API version",
            ));
        }
        validate_stable_identifier(&self.request_id, "Settings request ID")?;
        if matches!(
            self.command,
            SettingsCommandV1::GetProjection
                | SettingsCommandV1::TestSessionSummaryProvider { .. }
                | SettingsCommandV1::GetCodexNotifyStatus { .. }
                | SettingsCommandV1::ConfigureCodexNotify { .. }
        ) {
            if self.expected_revision.is_some() {
                return Err(ProtocolValidationError::InvalidIdentifier(
                    "Settings read revision",
                ));
            }
        } else if self.expected_revision.is_none() {
            return Err(ProtocolValidationError::InvalidIdentifier(
                "Settings mutation revision",
            ));
        }
        self.command.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SettingsResultV1 {
    Projection {
        projection: SettingsV1,
        theme_projection: Box<polar_station_settings::ThemeProjectionV1>,
    },
    ProviderTested {
        summary: String,
    },
    CodexNotifyStatus {
        execution_target_id: String,
        status: CodexNotifyConfigStateV1,
    },
    Error {
        error: ProtocolError,
        projection: Option<SettingsV1>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsResponseEnvelopeV1 {
    pub protocol_version: u32,
    pub api: SettingsApiV1,
    pub request_id: String,
    pub revision: u64,
    pub result: SettingsResultV1,
}

impl SettingsResponseEnvelopeV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.protocol_version != crate::PROTOCOL_VERSION || self.api != SettingsApiV1::SettingsV1
        {
            return Err(ProtocolValidationError::InvalidIdentifier(
                "Settings API version",
            ));
        }
        validate_stable_identifier(&self.request_id, "Settings request ID")?;
        match &self.result {
            SettingsResultV1::Projection { projection, .. } => projection
                .validate()
                .map_err(|_| ProtocolValidationError::InvalidIdentifier("Settings projection")),
            SettingsResultV1::ProviderTested { summary } => {
                if summary.is_empty()
                    || summary.len() > 256
                    || summary.chars().any(char::is_control)
                {
                    Err(ProtocolValidationError::InvalidIdentifier(
                        "Settings provider test summary",
                    ))
                } else {
                    Ok(())
                }
            }
            SettingsResultV1::CodexNotifyStatus {
                execution_target_id,
                ..
            } => validate_stable_identifier(execution_target_id, "Codex setup target"),
            SettingsResultV1::Error { error, projection } => {
                error.validate_sanitized()?;
                if let Some(value) = projection {
                    value.validate().map_err(|_| {
                        ProtocolValidationError::InvalidIdentifier("Settings projection")
                    })?;
                }
                Ok(())
            }
        }
    }
}
