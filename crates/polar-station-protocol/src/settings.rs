use polar_station_settings::{
    AppearanceSettingsV1, AttentionSettingsV1, MonitoringSettingsV1, SessionSummarySettingsV1,
    SettingsV1,
    TmuxStatusBarSettings, TmuxStatusFacts,
};
use serde::{Deserialize, Serialize, Serializer};

use crate::{validate_stable_identifier, PaneContentBindingV1, ProtocolError, ProtocolValidationError};

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
pub struct TmuxStatusPurposeEditV1 {
    pub pane: PaneContentBindingV1,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SettingsCommandV1 {
    GetProjection {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tmux_preview: Option<PaneContentBindingV1>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace_name: Option<String>,
    },
    UpdateAppearance { value: AppearanceSettingsV1 },
    UpdateAttention { value: AttentionSettingsV1 },
    UpdateMonitoring { value: MonitoringSettingsV1 },
    UpdateSessionSummary { value: SessionSummarySettingsV1 },
    UpdateTmuxStatusBar {
        value: TmuxStatusBarSettings,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        workspace_sessions: Vec<PaneContentBindingV1>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tmux_purpose: Option<Box<TmuxStatusPurposeEditV1>>,
    },
    TestSessionSummaryProvider { value: SessionSummarySettingsV1 },
    GetCodexNotifyStatus { execution_target_id: String },
    ConfigureCodexNotify { execution_target_id: String },
}

impl SettingsCommandV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if let Self::UpdateTmuxStatusBar { workspace_sessions, workspace_name, tmux_purpose, .. } = self {
            validate_status_text(workspace_name.as_deref(), "Workspace name")?;
            workspace_sessions.iter().try_for_each(PaneContentBindingV1::validate)?;
            if let Some(edit) = tmux_purpose {
                validate_status_text(Some(&edit.text), "Task or purpose")?;
                if !workspace_sessions.contains(&edit.pane) {
                    return Err(ProtocolValidationError::InvalidIdentifier("Purpose Session"));
                }
            }
            return Ok(());
        }
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
            Self::GetProjection { tmux_preview, workspace_name } => {
                validate_status_text(workspace_name.as_deref(), "Workspace name")?;
                return tmux_preview.as_ref().map_or(Ok(()), PaneContentBindingV1::validate);
            }
            Self::UpdateAppearance { value } => value.validate(),
            Self::UpdateAttention { .. } => Ok(()),
            Self::UpdateMonitoring { value } => value.validate(),
            Self::UpdateSessionSummary { value } | Self::TestSessionSummaryProvider { value } => {
                value.validate()
            }
            Self::GetCodexNotifyStatus { .. } | Self::ConfigureCodexNotify { .. }
            | Self::UpdateTmuxStatusBar { .. } => unreachable!(),
        };
        result.map_err(|_| ProtocolValidationError::InvalidIdentifier("Settings value"))
    }
}

fn validate_status_text(value: Option<&str>, label: &'static str) -> Result<(), ProtocolValidationError> {
    if value.is_some_and(|text| text.chars().count() > 128 || text.chars().any(char::is_control)) {
        return Err(ProtocolValidationError::InvalidIdentifier(label));
    }
    Ok(())
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
            SettingsCommandV1::GetProjection { .. }
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
        #[serde(serialize_with = "serialize_core_projection")]
        projection: SettingsV1,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tmux_status_bar: Option<TmuxStatusBarSettings>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tmux_status_warning: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tmux_status_facts: Option<TmuxStatusFacts>,
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
        #[serde(serialize_with = "serialize_optional_core_projection")]
        projection: Option<SettingsV1>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tmux_status_bar: Option<TmuxStatusBarSettings>,
    },
}

// Old Rust clients reject additional fields inside SettingsV1. Extensions
// belong in the result object, whose fields are forward-compatible.
fn core_projection(projection: &SettingsV1) -> serde_json::Value {
    let mut value = serde_json::json!(projection);
    if let Some(fields) = value.as_object_mut() {
        fields.remove("tmux_status_bar");
    }
    value
}

fn serialize_core_projection<S: Serializer>(projection: &SettingsV1, s: S) -> Result<S::Ok, S::Error> {
    core_projection(projection).serialize(s)
}

fn serialize_optional_core_projection<S: Serializer>(
    projection: &Option<SettingsV1>,
    s: S,
) -> Result<S::Ok, S::Error> {
    projection.as_ref().map(core_projection).serialize(s)
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
            SettingsResultV1::Error { error, projection, .. } => {
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
