//! Session runtime and raw terminal bridge contracts.
//!
//! These types deliberately stop at tmux Session identity and opaque terminal
//! Consumer identity. Workspace, Tab, Pane, layout, selection, focus, and
//! Ghostty Surface identity belong to the native application and must not cross
//! this boundary.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{
    validate_display_label, validate_stable_identifier, ExecutionTarget, PaneContentBindingV1,
    ProtocolError, ProtocolValidationError, SessionAttentionProjectionV1, SshTransportGenerationV1,
    MAX_APPLICATION_PAYLOAD_BYTES, PROTOCOL_VERSION,
};

pub const SESSION_RUNTIME_API_VERSION: u16 = 1;

const TERMINAL_FRAME_INPUT: u8 = 1;
const TERMINAL_FRAME_OUTPUT: u8 = 2;
const TERMINAL_FRAME_RESIZE: u8 = 3;
const TERMINAL_FRAME_BIND: u8 = 4;
const TERMINAL_FRAME_DETACH: u8 = 5;
const TERMINAL_FRAME_RETIRE: u8 = 6;
const TERMINAL_FRAME_STATUS: u8 = 7;
const TERMINAL_FRAME_HEADER_BYTES: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRuntimeApiV1 {
    SessionRuntimeV1,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionKeyV1 {
    pub execution_target: ExecutionTarget,
    pub tmux_session_id: String,
}

impl SessionKeyV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        self.execution_target.validate()?;
        validate_stable_identifier(&self.tmux_session_id, "tmux Session ID")
    }
}

impl From<PaneContentBindingV1> for SessionKeyV1 {
    fn from(binding: PaneContentBindingV1) -> Self {
        Self {
            execution_target: binding.execution_target,
            tmux_session_id: binding.tmux_session_id,
        }
    }
}

impl From<SessionKeyV1> for PaneContentBindingV1 {
    fn from(key: SessionKeyV1) -> Self {
        Self {
            execution_target: key.execution_target,
            tmux_session_id: key.tmux_session_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionLifecycleV1 {
    Connecting,
    Ready,
    Recovering,
    Unavailable,
    Ending,
    Ended,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionProjectionV1 {
    pub key: SessionKeyV1,
    pub display_name: String,
    pub lifecycle: SessionLifecycleV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_generation: Option<SshTransportGenerationV1>,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<SessionAttentionProjectionV1>,
}

impl SessionProjectionV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        self.key.validate()?;
        validate_display_label(&self.display_name, "Session display name")?;
        require_positive(self.revision, "Session revision")?;
        if let Some(generation) = &self.transport_generation {
            generation.validate()?;
        }
        if let Some(attention) = &self.attention {
            attention.validate()?;
            if attention.execution_target_id != self.key.execution_target.as_str()
                || attention.tmux_session_id != self.key.tmux_session_id
            {
                return Err(ProtocolValidationError::InvalidIdentifier(
                    "Session attention identity",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionFeedSnapshotV1 {
    pub epoch: String,
    pub revision: u64,
    pub sessions: Vec<SessionProjectionV1>,
}

impl SessionFeedSnapshotV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_feed_position(&self.epoch, self.revision)?;
        let mut keys = BTreeSet::new();
        for session in &self.sessions {
            session.validate()?;
            if session.revision > self.revision || !keys.insert(session.key.clone()) {
                return Err(ProtocolValidationError::InvalidIdentifier(
                    "Session feed snapshot",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // Frozen wire construction stays source-compatible.
pub enum SessionFeedChangeV1 {
    Upsert { session: SessionProjectionV1 },
    Remove { key: SessionKeyV1 },
}

impl SessionFeedChangeV1 {
    pub fn validate(&self, event_revision: u64) -> Result<(), ProtocolValidationError> {
        match self {
            Self::Upsert { session } => {
                session.validate()?;
                if session.revision != event_revision {
                    return Err(ProtocolValidationError::InvalidIdentifier(
                        "Session feed event revision",
                    ));
                }
            }
            Self::Remove { key } => key.validate()?,
        }
        Ok(())
    }

    fn key(&self) -> &SessionKeyV1 {
        match self {
            Self::Upsert { session } => &session.key,
            Self::Remove { key } => key,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionFeedEventV1 {
    pub epoch: String,
    pub revision: u64,
    pub changes: Vec<SessionFeedChangeV1>,
}

impl SessionFeedEventV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_feed_position(&self.epoch, self.revision)?;
        if self.changes.is_empty() {
            return Err(ProtocolValidationError::InvalidIdentifier(
                "Session feed event changes",
            ));
        }
        let mut keys = BTreeSet::new();
        for change in &self.changes {
            change.validate(self.revision)?;
            if !keys.insert(change.key().clone()) {
                return Err(ProtocolValidationError::InvalidIdentifier(
                    "Session feed event changes",
                ));
            }
        }
        Ok(())
    }
}

pub use polar_tmux_types::TerminalSizeV1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalBridgeHelloV1 {
    pub protocol_version: u32,
    pub session_runtime_api_version: u16,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub recovery_feedback: bool,
    #[serde(default = "initial_content_generation")]
    pub initial_content_generation: u64,
    pub consumer_id: String,
    pub consumer_generation: u64,
    pub terminal_size: TerminalSizeV1,
    pub terminal_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_binding: Option<PaneContentBindingV1>,
}

fn initial_content_generation() -> u64 {
    1
}

impl TerminalBridgeHelloV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ProtocolValidationError::UnsupportedProtocolVersion(
                self.protocol_version,
            ));
        }
        if self.session_runtime_api_version != SESSION_RUNTIME_API_VERSION {
            return Err(ProtocolValidationError::InvalidIdentifier(
                "Session runtime API version",
            ));
        }
        validate_stable_identifier(&self.consumer_id, "terminal Consumer ID")?;
        require_positive(self.consumer_generation, "terminal Consumer generation")?;
        require_positive(
            self.initial_content_generation,
            "initial content generation",
        )?;
        self.terminal_size.validate()?;
        validate_bounded_environment_value(&self.terminal_type, "terminal type", 128)?;
        if let Some(locale) = &self.locale {
            validate_bounded_environment_value(locale, "terminal locale", 128)?;
        }
        if let Some(binding) = &self.initial_binding {
            binding.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalConsumerLifecycleV1 {
    Detached,
    Connecting,
    Ready,
    Recovering,
    Unavailable,
    Ended,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalConsumerStatusV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<Box<TerminalRecoveryViewV1>>,
    pub consumer_id: String,
    pub consumer_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<PaneContentBindingV1>,
    pub lifecycle: TerminalConsumerLifecycleV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_generation: Option<SshTransportGenerationV1>,
    pub content_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtocolError>,
}

impl TerminalConsumerStatusV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_stable_identifier(&self.consumer_id, "terminal Consumer ID")?;
        require_positive(self.consumer_generation, "terminal Consumer generation")?;
        require_positive(self.content_generation, "terminal content generation")?;
        if let Some(binding) = &self.binding {
            binding.validate()?;
        }
        if let Some(generation) = &self.transport_generation {
            generation.validate()?;
        }
        if let Some(error) = &self.error {
            error.validate_sanitized()?;
        }
        match self.lifecycle {
            TerminalConsumerLifecycleV1::Detached if self.binding.is_some() => Err(
                ProtocolValidationError::InvalidIdentifier("detached terminal binding"),
            ),
            TerminalConsumerLifecycleV1::Ready if self.binding.is_none() => Err(
                ProtocolValidationError::InvalidIdentifier("ready terminal binding"),
            ),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalBridgeFrameV1 {
    Control(crate::TerminalControlIntentV1),
    ControlStatus(crate::TerminalControlStatusV1),
    Retry,
    Focus {
        focused: bool,
        binding_generation: u64,
        size: TerminalSizeV1,
    },
    DismissRecovery,
    RetainBinding {
        binding_generation: u64,
    },
    Input(Vec<u8>),
    Output(Vec<u8>),
    Resize(TerminalSizeV1),
    Bind {
        binding: PaneContentBindingV1,
        binding_generation: u64,
    },
    Detach {
        binding_generation: u64,
    },
    Retire,
    Status(TerminalConsumerStatusV1),
}

impl TerminalBridgeFrameV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::Control(intent) => intent.validate(),
            Self::ControlStatus(_) => Ok(()),
            Self::Input(bytes) | Self::Output(bytes) => {
                if bytes.len() > MAX_APPLICATION_PAYLOAD_BYTES {
                    return Err(ProtocolValidationError::InvalidIdentifier(
                        "terminal byte frame",
                    ));
                }
                Ok(())
            }
            Self::Resize(size) => size.validate(),
            Self::Focus {
                binding_generation,
                size,
                ..
            } => {
                require_positive(*binding_generation, "terminal binding generation")?;
                size.validate()
            }
            Self::Bind {
                binding,
                binding_generation,
            } => {
                binding.validate()?;
                require_positive(*binding_generation, "terminal binding generation")
            }
            Self::Detach { binding_generation } | Self::RetainBinding { binding_generation } => {
                require_positive(*binding_generation, "terminal binding generation")
            }
            Self::Retire | Self::Retry | Self::DismissRecovery => Ok(()),
            Self::Status(status) => status.validate(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalBridgeFrameError {
    Truncated,
    PayloadTooLarge,
    LengthMismatch,
    UnknownKind(u8),
    Json(String),
    Invalid(ProtocolValidationError),
}

impl fmt::Display for TerminalBridgeFrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => formatter.write_str("terminal bridge frame is truncated"),
            Self::PayloadTooLarge => formatter.write_str("terminal bridge frame exceeds 1 MiB"),
            Self::LengthMismatch => {
                formatter.write_str("terminal bridge frame length does not match its prefix")
            }
            Self::UnknownKind(kind) => {
                write!(formatter, "unknown terminal bridge frame kind {kind}")
            }
            Self::Json(message) => write!(
                formatter,
                "terminal bridge control frame is invalid: {message}"
            ),
            Self::Invalid(error) => write!(formatter, "terminal bridge frame is invalid: {error}"),
        }
    }
}

impl Error for TerminalBridgeFrameError {}

pub fn encode_terminal_bridge_frame_v1(
    frame: &TerminalBridgeFrameV1,
) -> Result<Vec<u8>, TerminalBridgeFrameError> {
    frame
        .validate()
        .map_err(TerminalBridgeFrameError::Invalid)?;
    let (kind, payload) = match frame {
        TerminalBridgeFrameV1::Control(intent) => (12, encode_control(intent)?),
        TerminalBridgeFrameV1::ControlStatus(state) => (13, encode_control(state)?),
        TerminalBridgeFrameV1::Focus {
            focused,
            binding_generation,
            size,
        } => (11, encode_control(&(focused, binding_generation, size))?),
        TerminalBridgeFrameV1::Retry => (8, Vec::new()),
        TerminalBridgeFrameV1::DismissRecovery => (9, Vec::new()),
        TerminalBridgeFrameV1::RetainBinding { binding_generation } => {
            (10, encode_control(binding_generation)?)
        }
        TerminalBridgeFrameV1::Input(bytes) => (TERMINAL_FRAME_INPUT, bytes.clone()),
        TerminalBridgeFrameV1::Output(bytes) => (TERMINAL_FRAME_OUTPUT, bytes.clone()),
        TerminalBridgeFrameV1::Resize(value) => (TERMINAL_FRAME_RESIZE, encode_control(value)?),
        TerminalBridgeFrameV1::Bind {
            binding,
            binding_generation,
        } => (
            TERMINAL_FRAME_BIND,
            encode_control(&(binding, binding_generation))?,
        ),
        TerminalBridgeFrameV1::Detach { binding_generation } => {
            (TERMINAL_FRAME_DETACH, encode_control(binding_generation)?)
        }
        TerminalBridgeFrameV1::Retire => (TERMINAL_FRAME_RETIRE, Vec::new()),
        TerminalBridgeFrameV1::Status(value) => (TERMINAL_FRAME_STATUS, encode_control(value)?),
    };
    if payload.len() > MAX_APPLICATION_PAYLOAD_BYTES {
        return Err(TerminalBridgeFrameError::PayloadTooLarge);
    }
    let length =
        u32::try_from(payload.len()).map_err(|_| TerminalBridgeFrameError::PayloadTooLarge)?;
    let mut encoded = Vec::with_capacity(TERMINAL_FRAME_HEADER_BYTES + payload.len());
    encoded.push(kind);
    encoded.extend_from_slice(&length.to_be_bytes());
    encoded.extend_from_slice(&payload);
    Ok(encoded)
}

pub fn decode_terminal_bridge_frame_v1(
    encoded: &[u8],
) -> Result<TerminalBridgeFrameV1, TerminalBridgeFrameError> {
    let kind = *encoded.first().ok_or(TerminalBridgeFrameError::Truncated)?;
    let length = u32::from_be_bytes(
        encoded
            .get(1..TERMINAL_FRAME_HEADER_BYTES)
            .ok_or(TerminalBridgeFrameError::Truncated)?
            .try_into()
            .expect("checked four-byte terminal frame prefix"),
    ) as usize;
    if length > MAX_APPLICATION_PAYLOAD_BYTES {
        return Err(TerminalBridgeFrameError::PayloadTooLarge);
    }
    if encoded.len() != TERMINAL_FRAME_HEADER_BYTES + length {
        return Err(TerminalBridgeFrameError::LengthMismatch);
    }
    let payload = &encoded[TERMINAL_FRAME_HEADER_BYTES..];
    let frame = match kind {
        12 => TerminalBridgeFrameV1::Control(decode_control(payload)?),
        13 => TerminalBridgeFrameV1::ControlStatus(decode_control(payload)?),
        11 => {
            let (focused, binding_generation, size) = decode_control(payload)?;
            TerminalBridgeFrameV1::Focus {
                focused,
                binding_generation,
                size,
            }
        }
        8 if payload.is_empty() => TerminalBridgeFrameV1::Retry,
        9 if payload.is_empty() => TerminalBridgeFrameV1::DismissRecovery,
        10 => TerminalBridgeFrameV1::RetainBinding {
            binding_generation: decode_control(payload)?,
        },
        TERMINAL_FRAME_INPUT => TerminalBridgeFrameV1::Input(payload.to_vec()),
        TERMINAL_FRAME_OUTPUT => TerminalBridgeFrameV1::Output(payload.to_vec()),
        TERMINAL_FRAME_RESIZE => TerminalBridgeFrameV1::Resize(decode_control(payload)?),
        TERMINAL_FRAME_BIND => {
            let (binding, binding_generation) = decode_control(payload)?;
            TerminalBridgeFrameV1::Bind {
                binding,
                binding_generation,
            }
        }
        TERMINAL_FRAME_DETACH => TerminalBridgeFrameV1::Detach {
            binding_generation: decode_control(payload)?,
        },
        TERMINAL_FRAME_RETIRE => {
            if !payload.is_empty() {
                return Err(TerminalBridgeFrameError::LengthMismatch);
            }
            TerminalBridgeFrameV1::Retire
        }
        TERMINAL_FRAME_STATUS => TerminalBridgeFrameV1::Status(decode_control(payload)?),
        other => return Err(TerminalBridgeFrameError::UnknownKind(other)),
    };
    frame
        .validate()
        .map_err(TerminalBridgeFrameError::Invalid)?;
    Ok(frame)
}

fn encode_control<T: Serialize>(value: &T) -> Result<Vec<u8>, TerminalBridgeFrameError> {
    serde_json::to_vec(value).map_err(|error| TerminalBridgeFrameError::Json(error.to_string()))
}

fn decode_control<T: DeserializeOwned>(payload: &[u8]) -> Result<T, TerminalBridgeFrameError> {
    serde_json::from_slice(payload)
        .map_err(|error| TerminalBridgeFrameError::Json(error.to_string()))
}

fn validate_feed_position(epoch: &str, revision: u64) -> Result<(), ProtocolValidationError> {
    validate_stable_identifier(epoch, "Session feed epoch")?;
    require_positive(revision, "Session feed revision")
}

fn require_positive(value: u64, field: &'static str) -> Result<(), ProtocolValidationError> {
    if value == 0 {
        return Err(ProtocolValidationError::InvalidIdentifier(field));
    }
    Ok(())
}

fn validate_bounded_environment_value(
    value: &str,
    field: &'static str,
    max_bytes: usize,
) -> Result<(), ProtocolValidationError> {
    if value.is_empty()
        || value.len() > max_bytes
        || value != value.trim()
        || value.chars().any(|character| character.is_control())
    {
        return Err(ProtocolValidationError::InvalidIdentifier(field));
    }
    Ok(())
}

pub use polar_tmux_types::{TerminalRecoveryPhaseV1, TerminalRecoveryViewV1};
