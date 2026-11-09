use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustError {
    Signing(polar_identity::SignerError),
    InvalidRequest,
    InvalidDecision,
    InvalidSignature,
    InvalidStateTransition,
    IdentityMismatch,
    BindingMismatch,
    TimestampOutOfRange,
}

impl fmt::Display for TrustError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Signing(err) => return err.fmt(f),
            Self::InvalidRequest => "invalid trust request",
            Self::InvalidDecision => "invalid trust decision",
            Self::InvalidSignature => "invalid signature",
            Self::InvalidStateTransition => "invalid state transition",
            Self::IdentityMismatch => "identity mismatch",
            Self::BindingMismatch => "binding mismatch",
            Self::TimestampOutOfRange => "timestamp out of range",
        };
        f.write_str(text)
    }
}

impl Error for TrustError {}

impl From<polar_identity::SignerError> for TrustError {
    fn from(value: polar_identity::SignerError) -> Self {
        Self::Signing(value)
    }
}
