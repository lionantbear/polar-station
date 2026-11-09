//! Content-only contract between a native Polar Station frontend and Rust.
//!
//! A native frontend owns windows, tabs, surfaces, layout, focus, and every
//! presentation mutation. Rust receives only the content binding requested for
//! one already-existing physical channel and owns the tmux/SSH work needed to
//! make that binding ready.

use serde::{Deserialize, Serialize};

use crate::{
    validate_stable_identifier, ExecutionTarget, ProtocolError, ProtocolValidationError,
    PROTOCOL_VERSION,
};

pub const PANE_CONTENT_API_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneContentApiV1 {
    PaneContentV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneContentBindingV1 {
    pub execution_target: ExecutionTarget,
    pub tmux_session_id: String,
}

impl PaneContentBindingV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        self.execution_target.validate()?;
        validate_stable_identifier(&self.tmux_session_id, "tmux Session ID")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum PaneContentCommandV1 {
    ObserveRecovery,
    UnobserveRecovery,
    ProbeRecovery,
    Focus {
        focused: bool,
        sequence: u64,
    },
    TerminalControl {
        native_instance_id: String,
        focused: bool,
        sequence: u64,
        #[serde(default)]
        cause: crate::TerminalControlCauseV1,
    },
    ProbeTerminalControl,
    DismissRecovery,
    Bind {
        binding: PaneContentBindingV1,
    },
    Detach,
    Retire,
    Reconnect {
        binding: PaneContentBindingV1,
    },
}

impl PaneContentCommandV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::Bind { binding } | Self::Reconnect { binding } => binding.validate(),
            Self::TerminalControl {
                native_instance_id,
                sequence,
                ..
            } => {
                validate_stable_identifier(native_instance_id, "native control instance")?;
                if *sequence == 0 {
                    return Err(ProtocolValidationError::InvalidIdentifier(
                        "control sequence",
                    ));
                }
                Ok(())
            }
            Self::ProbeTerminalControl => Ok(()),
            Self::Focus { sequence, .. } if *sequence == 0 => {
                Err(ProtocolValidationError::InvalidIdentifier("focus sequence"))
            }
            Self::Focus { .. }
            | Self::Detach
            | Self::Retire
            | Self::ObserveRecovery
            | Self::UnobserveRecovery
            | Self::ProbeRecovery
            | Self::DismissRecovery => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneContentRequestV1 {
    pub protocol_version: u32,
    pub api: PaneContentApiV1,
    pub request_id: String,
    pub channel_id: String,
    pub channel_generation: u64,
    pub content_revision: u64,
    pub command: PaneContentCommandV1,
}

impl PaneContentRequestV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_envelope(
            self.protocol_version,
            &self.request_id,
            &self.channel_id,
            self.channel_generation,
            self.content_revision,
        )?;
        self.command.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PaneContentResultV1 {
    TerminalControl {
        state: crate::TerminalControlStatusV1,
    },
    Recovery {
        view: Box<crate::TerminalRecoveryViewV1>,
    },
    /// Desired binding accepted; readiness arrives on the status subscription.
    Accepted {
        binding: PaneContentBindingV1,
    },
    Ready {
        binding: PaneContentBindingV1,
    },
    Detached,
    Rejected {
        error: ProtocolError,
    },
}

impl PaneContentResultV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::Ready { binding } | Self::Accepted { binding } => binding.validate(),
            Self::Detached | Self::Recovery { .. } | Self::TerminalControl { .. } => Ok(()),
            Self::Rejected { error } => error.validate_sanitized(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneContentResponseV1 {
    pub protocol_version: u32,
    pub api: PaneContentApiV1,
    pub request_id: String,
    pub channel_id: String,
    pub channel_generation: u64,
    pub content_revision: u64,
    pub result: PaneContentResultV1,
}

impl PaneContentResponseV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_envelope(
            self.protocol_version,
            &self.request_id,
            &self.channel_id,
            self.channel_generation,
            self.content_revision,
        )?;
        self.result.validate()
    }
}

fn validate_envelope(
    protocol_version: u32,
    request_id: &str,
    channel_id: &str,
    channel_generation: u64,
    content_revision: u64,
) -> Result<(), ProtocolValidationError> {
    if protocol_version != PROTOCOL_VERSION {
        return Err(ProtocolValidationError::UnsupportedProtocolVersion(
            protocol_version,
        ));
    }
    validate_stable_identifier(request_id, "content request ID")?;
    validate_stable_identifier(channel_id, "content channel ID")?;
    if channel_generation == 0 {
        return Err(ProtocolValidationError::InvalidIdentifier(
            "content channel generation",
        ));
    }
    if content_revision == 0 {
        return Err(ProtocolValidationError::InvalidIdentifier(
            "content revision",
        ));
    }
    Ok(())
}
