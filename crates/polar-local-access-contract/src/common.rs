use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl ProtocolError {
    pub fn new(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
        }
    }

    pub fn validate_sanitized(&self) -> Result<(), ProtocolValidationError> {
        validate_stable_identifier(&self.code, "error code")?;
        if self.message.is_empty()
            || self.message.len() > 512
            || self.message.chars().any(char::is_control)
        {
            return Err(ProtocolValidationError::UnsanitizedError);
        }
        Ok(())
    }
}

pub fn validate_stable_identifier(
    value: &str,
    field: &'static str,
) -> Result<(), ProtocolValidationError> {
    if value.is_empty()
        || value.len() > 256
        || value != value.trim()
        || value.chars().any(char::is_control)
    {
        return Err(ProtocolValidationError::InvalidIdentifier(field));
    }
    Ok(())
}

pub fn validate_display_label(
    value: &str,
    field: &'static str,
) -> Result<(), ProtocolValidationError> {
    if value.is_empty()
        || value.len() > 128
        || value != value.trim()
        || value.chars().any(char::is_control)
    {
        return Err(ProtocolValidationError::InvalidIdentifier(field));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolValidationError {
    UnsupportedProtocolVersion(u32),
    InvalidIdentifier(&'static str),
    InvalidBase64(&'static str),
    MissingExpectedRevision,
    InvalidWorkspaceScope,
    InvalidWorkspaceCommand,
    InvalidControlCenterCommand,
    InvalidResourcesCommand,
    InvalidAttachPlan,
    UnsanitizedError,
}

impl fmt::Display for ProtocolValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedProtocolVersion(version) => {
                write!(formatter, "protocol version {version} is not supported")
            }
            Self::InvalidIdentifier(field) => write!(formatter, "{field} is invalid"),
            Self::InvalidBase64(field) => {
                write!(formatter, "{field} is not canonical 32-byte base64")
            }
            Self::MissingExpectedRevision => {
                formatter.write_str("mutation requires its expected revision")
            }
            Self::InvalidWorkspaceScope => formatter.write_str("Workspace scope is invalid"),
            Self::InvalidWorkspaceCommand => formatter.write_str("Workspace command is invalid"),
            Self::InvalidControlCenterCommand => {
                formatter.write_str("Control Center command or projection is invalid")
            }
            Self::InvalidResourcesCommand => {
                formatter.write_str("Resources command or projection is invalid")
            }
            Self::InvalidAttachPlan => formatter.write_str("tmux attach plan is invalid"),
            Self::UnsanitizedError => {
                formatter.write_str("protocol error message is not sanitized")
            }
        }
    }
}

impl Error for ProtocolValidationError {}
