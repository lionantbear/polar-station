use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    InvalidHex,
    InvalidLength,
    InvalidNodeName,
    InvalidSignature,
    InvalidTimestamp,
    InvalidBinding,
    CanonicalizationError,
    IdentityMismatch,
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::InvalidHex => "invalid hex",
            Self::InvalidLength => "invalid length",
            Self::InvalidNodeName => "invalid node name",
            Self::InvalidSignature => "invalid signature",
            Self::InvalidTimestamp => "invalid timestamp",
            Self::InvalidBinding => "invalid binding",
            Self::CanonicalizationError => "canonicalization error",
            Self::IdentityMismatch => "identity mismatch",
        };
        f.write_str(text)
    }
}

impl Error for IdentityError {}
