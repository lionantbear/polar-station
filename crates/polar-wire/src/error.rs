use std::fmt;

#[derive(Debug)]
pub enum WireError {
    Codec(String),
    BodyKindMismatch { header: u16, body_box: u16 },
    MissingRawPacket,
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Codec(error) => write!(f, "wire codec error: {error}"),
            Self::BodyKindMismatch { header, body_box } => write!(
                f,
                "wire body kind mismatch: header={header} body_box={body_box}"
            ),
            Self::MissingRawPacket => write!(f, "raw packet bytes are not available"),
        }
    }
}

impl std::error::Error for WireError {}

impl From<serde_cbor::Error> for WireError {
    fn from(value: serde_cbor::Error) -> Self {
        Self::Codec(value.to_string())
    }
}

pub type WireResult<T> = Result<T, WireError>;
