//! Original-thread Codex control. Neither terminal input nor task scheduling lives here.
use crate::{validate_stable_identifier, ProtocolValidationError, SessionKeyV1, PROTOCOL_VERSION};
pub use polar_station_inbox::{CodexCompletion, CodexCompletionCursor};
use polar_station_settings::CodexControlSettingsV1;
use serde::{Deserialize, Serialize};

pub const CODEX_CONTROL_API_VERSION: u16 = 2;
pub const CODEX_INPUT_MAX_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexApiV1 {
    CodexV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexBindingV1 {
    pub owner_host_id: String,
    pub binding_id: String,
    pub revision: u64,
    pub session: Option<SessionKeyV1>,
    pub endpoint_id: String,
    pub thread_id: String,
    pub telegram_destination_ref: Option<String>,
    pub imessage_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexConnectionStateV1 {
    Unsupported,
    Unbound,
    Connecting,
    Connected,
    Disconnected,
    Stale,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexActivityV1 {
    Idle,
    Active,
    WaitingApproval,
    WaitingInput,
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexThreadMetadataV1 {
    pub preview: Option<String>,
    pub cwd: Option<String>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub parent_thread_id: Option<String>,
    pub agent_role: Option<String>,
    pub agent_nickname: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexStateV1 {
    #[serde(default)]
    pub metadata: Box<CodexThreadMetadataV1>,
    #[serde(default)]
    pub thread_name: Option<String>,
    pub binding: CodexBindingV1,
    pub connection: CodexConnectionStateV1,
    pub activity: CodexActivityV1,
    pub turn_id: Option<String>,
    pub final_answer: Option<String>,
    pub error_code: Option<String>,
    pub can_queue: bool,
    pub can_steer: bool,
    pub can_interrupt: bool,
    #[serde(default)]
    pub presentation: Option<CodexSessionPresentationV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexSessionPresentationV1 {
    pub connection_label: String,
    pub task_label: String,
    pub detail: Option<String>,
    pub input_label: String,
    pub actions: Vec<crate::ControlActionProjectionV1>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexSubmissionStatusV1 {
    Received,
    Submitting,
    Accepted,
    Rejected,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexSubmissionReceiptV1 {
    pub intent_id: String,
    pub status: CodexSubmissionStatusV1,
    pub queue_id: Option<String>,
    pub turn_id: Option<String>,
    pub error_code: Option<String>,
    pub label: String,
    pub clear_input: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexLoadedThreadV1 {
    pub thread_id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CodexCommandV1 {
    GetUi,
    StartLocalServer { endpoint_id:String },
    GetSettings,
    GetTelegramStatus,
    SetupCurrentTelegram {
        expected_revision: u64,
        allowed_sender_id: Option<i64>,
    },
    SetTelegramEnabled {
        expected_revision: u64,
        enabled: bool,
    },
    ConnectImessageSender {
        expected_revision: u64,
        ssh_target_id: String,
        port: u16,
        pairing: crate::NotificationPairingV1,
    },
    UpdateSettings {
        expected_revision: u64,
        settings: CodexControlSettingsV1,
    },
    StartThread {
        endpoint_id: String,
        #[serde(default)]
        name: Option<String>,
        working_directory: Option<String>,
        #[serde(default)]
        yolo: bool,
    },
    /// Observe and resume an existing original Thread, without a terminal.
    OpenThread { endpoint_id: String, thread_id: String },
    /// Submit visible text to exactly this original Thread; resume only when unloaded.
    SubmitThreadText { endpoint_id: String, thread_id: String, intent_id: String, text: String },
    GetThreadBinding { endpoint_id: String, thread_id: String },
    ReadThread {
        endpoint_id: String,
        thread_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_reference: Option<String>,
    },
    ReadHistoryDetail { endpoint_id: String, thread_id: String },
    RenameThread { endpoint_id: String, thread_id: String, name: String },
    ReadDirectory { endpoint_id: String, path: String },
    /// Read persisted history from one endpoint without loading any Thread.
    ListHistory {
        endpoint_id: String,
        cursor: Option<String>,
        search_term: Option<String>,
        #[serde(default)]
        archived: bool,
        #[serde(default)]
        include_subagents: bool,
    },
    ListLoaded {
        endpoint_id: String,
    },
    Bind {
        session: SessionKeyV1,
        endpoint_id: String,
        thread_id: String,
    },
    Unbind {
        binding_id: String,
        expected_revision: u64,
    },
    ReleaseThread { binding_id: String, expected_revision: u64, turn_id: Option<String> },
    GetBindings,
    GetCompletions {
        endpoint_id: Option<String>,
        before: Option<CodexCompletionCursor>,
        limit: u16,
    },
    GetBinding {
        binding_id: String,
    },
    Resume {
        binding_id: String,
        expected_revision: u64,
    },
    UpdateNotifications {
        binding_id: String,
        expected_revision: u64,
        telegram_destination_ref: Option<String>,
        imessage_enabled: bool,
    },
    QueueText {
        binding_id: String,
        expected_revision: u64,
        intent_id: String,
        text: String,
    },
    Steer {
        binding_id: String,
        expected_revision: u64,
        intent_id: String,
        expected_turn_id: String,
        text: String,
    },
    Interrupt {
        binding_id: String,
        expected_revision: u64,
        intent_id: String,
        turn_id: String,
    },
    GetReceipt {
        intent_id: String,
    },
}

pub fn validate_codex_input(text: &str) -> Result<(), ProtocolValidationError> {
    if text.trim().is_empty()
        || text.len() > CODEX_INPUT_MAX_BYTES
        || text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(ProtocolValidationError::InvalidIdentifier("Codex text"));
    }
    Ok(())
}

impl CodexCommandV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::ReleaseThread { binding_id, turn_id, .. } => {
                validate_stable_identifier(binding_id, "Codex binding")?;
                if let Some(id) = turn_id { validate_stable_identifier(id, "Codex turn")?; }
                Ok(())
            }
            Self::GetUi | Self::GetSettings | Self::GetBindings | Self::GetTelegramStatus => Ok(()),
            Self::SetupCurrentTelegram {
                allowed_sender_id, ..
            } => {
                if allowed_sender_id.is_some_and(|id| id <= 0) {
                    return Err(ProtocolValidationError::InvalidIdentifier(
                        "Telegram sender",
                    ));
                }
                Ok(())
            }
            Self::SetTelegramEnabled { .. } => Ok(()),
            Self::ConnectImessageSender {
                ssh_target_id,
                port,
                pairing,
                ..
            } => {
                validate_stable_identifier(ssh_target_id, "SSH target")?;
                if *port == 0 {
                    return Err(ProtocolValidationError::InvalidIdentifier("Host port"));
                }
                pairing.validate()
            }
            Self::GetCompletions {
                endpoint_id,
                before,
                limit,
            } => {
                if !(1..=32).contains(limit)
                    || before.as_ref().is_some_and(|c| c.validate().is_err())
                {
                    return Err(ProtocolValidationError::InvalidIdentifier(
                        "Codex completion page",
                    ));
                }
                if let Some(id) = endpoint_id {
                    validate_stable_identifier(id, "Codex endpoint")?;
                }
                Ok(())
            }
            Self::UpdateSettings { settings, .. } => settings
                .validate()
                .map_err(|_| ProtocolValidationError::InvalidIdentifier("Codex settings")),
            Self::RenameThread { endpoint_id, thread_id, name } => {
                validate_stable_identifier(endpoint_id, "Codex endpoint")?;
                validate_stable_identifier(thread_id, "Codex Thread ID")?;
                if name.trim().is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
                    return Err(ProtocolValidationError::InvalidIdentifier("thread name"));
                }
                Ok(())
            }
            Self::ReadDirectory { endpoint_id, path } => {
                validate_stable_identifier(endpoint_id, "Codex endpoint")?;
                if !path.starts_with('/') || path.len() > 4096 || path.chars().any(char::is_control) {
                    return Err(ProtocolValidationError::InvalidIdentifier("directory path"));
                }
                Ok(())
            }
            Self::StartThread { endpoint_id, name, working_directory, .. } => {
                validate_stable_identifier(endpoint_id, "Codex endpoint")?;
                if name.as_ref().is_some_and(|v| v.trim().is_empty() || v.len() > 256 || v.chars().any(char::is_control)) {
                    return Err(ProtocolValidationError::InvalidIdentifier("thread name"));
                }
                if working_directory.as_ref().is_some_and(|v| v.is_empty() || v.len()>4096 || v.chars().any(char::is_control)) {
                    return Err(ProtocolValidationError::InvalidIdentifier("working directory"));
                }
                Ok(())
            }
            Self::OpenThread { endpoint_id, thread_id }
            | Self::GetThreadBinding { endpoint_id, thread_id }
            | Self::ReadThread { endpoint_id, thread_id, .. }
            | Self::ReadHistoryDetail { endpoint_id, thread_id } => {
                validate_stable_identifier(endpoint_id, "Codex endpoint")?;
                if let Self::ReadThread { file_reference: Some(path), .. } = self {
                    if path.is_empty() || path.len() > 16384 || path.chars().any(char::is_control) {
                        return Err(ProtocolValidationError::InvalidIdentifier("source file reference"));
                    }
                }
                validate_stable_identifier(thread_id, "Codex Thread ID")
            }
            Self::ListHistory { endpoint_id, cursor, search_term, .. } => {
                validate_stable_identifier(endpoint_id, "Codex endpoint")?;
                for value in [cursor, search_term].into_iter().flatten() {
                    if value.len() > 4096 || value.chars().any(char::is_control) {
                        return Err(ProtocolValidationError::InvalidControlCenterCommand);
                    }
                }
                Ok(())
            }
            Self::StartLocalServer { endpoint_id } | Self::ListLoaded { endpoint_id } => {
                validate_stable_identifier(endpoint_id, "Codex endpoint")
            }
            Self::Bind {
                session,
                endpoint_id,
                thread_id,
            } => {
                session.validate()?;
                validate_stable_identifier(endpoint_id, "Codex endpoint")?;
                validate_stable_identifier(thread_id, "Codex thread")
            }
            Self::GetBinding { binding_id }
            | Self::Resume { binding_id, .. }
            | Self::Unbind { binding_id, .. } => {
                validate_stable_identifier(binding_id, "Codex binding")
            }
            Self::UpdateNotifications {
                binding_id,
                telegram_destination_ref,
                ..
            } => {
                validate_stable_identifier(binding_id, "Codex binding")?;
                if let Some(id) = telegram_destination_ref {
                    validate_stable_identifier(id, "Telegram destination")?;
                }
                Ok(())
            }
            Self::SubmitThreadText { endpoint_id, thread_id, intent_id, text } => {
                validate_stable_identifier(endpoint_id, "Codex endpoint")?;
                validate_stable_identifier(thread_id, "Codex Thread ID")?;
                validate_stable_identifier(intent_id, "Codex intent")?;
                validate_codex_input(text)
            }
            Self::QueueText {
                binding_id,
                intent_id,
                text,
                ..
            }
            | Self::Steer {
                binding_id,
                intent_id,
                text,
                ..
            } => {
                validate_stable_identifier(binding_id, "Codex binding")?;
                validate_stable_identifier(intent_id, "Codex intent")?;
                if let Self::Steer {
                    expected_turn_id, ..
                } = self
                {
                    validate_stable_identifier(expected_turn_id, "Codex turn")?;
                }
                validate_codex_input(text)
            }
            Self::Interrupt {
                binding_id,
                intent_id,
                turn_id,
                ..
            } => {
                validate_stable_identifier(binding_id, "Codex binding")?;
                validate_stable_identifier(intent_id, "Codex intent")?;
                validate_stable_identifier(turn_id, "Codex turn")
            }
            Self::GetReceipt { intent_id } => validate_stable_identifier(intent_id, "Codex intent"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexRequestEnvelopeV1 {
    pub protocol_version: u32,
    pub api: CodexApiV1,
    pub request_id: String,
    pub owner_host_id: String,
    pub command: CodexCommandV1,
}
impl CodexRequestEnvelopeV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if serde_json::to_vec(self).map_or(true, |v| v.len() > 64 * 1024) {
            return Err(ProtocolValidationError::InvalidIdentifier(
                "Codex request size",
            ));
        }
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ProtocolValidationError::InvalidIdentifier(
                "Codex protocol version",
            ));
        }
        validate_stable_identifier(&self.request_id, "Codex request")?;
        validate_stable_identifier(&self.owner_host_id, "Codex owner")?;
        self.command.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CodexResultV1 {
    Ui {
        labels: std::collections::BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        file_reference_lookup: bool,
    },
    Completions {
        completions: Vec<CodexCompletionProjectionV1>,
        next_before: Option<CodexCompletionCursor>,
    },
    TelegramStatus {
        status: CodexChannelStatusV1,
    },
    Settings {
        settings: CodexControlSettingsV1,
    },
    Thread {
        thread: CodexHistoryThreadV1,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resolved_file_reference: Option<String>,
    },
    HistoryDetail { detail: CodexHistoryDetailV1 },
    Directories { paths: Vec<String> },
    History {
        threads: Vec<CodexHistoryThreadV1>,
        next_cursor: Option<String>,
    },
    Loaded {
        threads: Vec<CodexLoadedThreadV1>,
    },
    Bindings {
        bindings: Vec<CodexStateV1>,
    },
    Binding {
        binding: Option<CodexStateV1>,
    },
    Receipt {
        receipt: Option<CodexSubmissionReceiptV1>,
    },
    Error {
        code: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexHistoryThreadV1 {
    #[serde(default)]
    pub source_path: Option<String>,
    #[serde(default)]
    pub preview: Option<String>,
    pub thread_id: String,
    pub display_name: String,
    pub cwd: String,
    pub updated_at: i64,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexCompletionProjectionV1 {
    pub completion: CodexCompletion,
    pub deliveries: Vec<CodexDeliveryProjectionV1>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexDeliveryProjectionV1 {
    pub channel: String,
    pub status: String,
    pub label: String,
    pub created_at_ms: u64,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexChannelStatusV1 {
    pub state: CodexChannelStateV1,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexChannelStateV1 {
    Disabled,
    Connecting,
    Ready,
    Error,
    Unsupported,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexResponseEnvelopeV1 {
    pub protocol_version: u32,
    pub api: CodexApiV1,
    pub request_id: String,
    pub owner_host_id: String,
    pub result: CodexResultV1,
}

impl CodexResponseEnvelopeV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.protocol_version != PROTOCOL_VERSION
            || serde_json::to_vec(self).map_or(true, |v| v.len() > 512 * 1024)
        {
            return Err(ProtocolValidationError::InvalidIdentifier("Codex response"));
        }
        validate_stable_identifier(&self.request_id, "Codex request")?;
        validate_stable_identifier(&self.owner_host_id, "Codex owner")
    }
}

/// Read-only text for the selected history row. Never a generated summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexHistoryDetailV1 {
    pub thread: CodexHistoryThreadV1,
    pub turn_id: Option<String>,
    pub turn_status: Option<String>,
    pub user_text: Option<String>,
    pub assistant_text: Option<String>,
}


