use std::error::Error;
use std::fmt;

use polar_identity::IdentityError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    Signing(polar_identity::SignerError),
    Identity(IdentityError),
    InvalidSignature,
    InvalidStatus,
    InvalidIntent,
    InvalidMessage,
    InvalidCapability,
    InvalidTarget,
    InvalidSecret,
    InvalidEpoch,
    IdentityMismatch,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Signing(err) => return err.fmt(f),
            Self::Identity(err) => return err.fmt(f),
            Self::InvalidSignature => "invalid signature",
            Self::InvalidStatus => "invalid status",
            Self::InvalidIntent => "invalid intent",
            Self::InvalidMessage => "invalid message",
            Self::InvalidCapability => "invalid capability",
            Self::InvalidTarget => "invalid target",
            Self::InvalidSecret => "invalid secret",
            Self::InvalidEpoch => "invalid epoch",
            Self::IdentityMismatch => "identity mismatch",
        };
        f.write_str(text)
    }
}

impl Error for ProtocolError {}

impl From<IdentityError> for ProtocolError {
    fn from(value: IdentityError) -> Self {
        Self::Identity(value)
    }
}

impl From<polar_identity::SignerError> for ProtocolError {
    fn from(value: polar_identity::SignerError) -> Self {
        Self::Signing(value)
    }
}
