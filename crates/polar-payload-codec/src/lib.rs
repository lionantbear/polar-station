use polar_protocol::MessagePayload;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessagePayloadFamily {
    Text,
    Json,
    Binary,
}

impl MessagePayloadFamily {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
            Self::Binary => "binary",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "text" => Some(Self::Text),
            "json" => Some(Self::Json),
            "binary" => Some(Self::Binary),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessagePayloadEncoding {
    Utf8,
    Json,
    Cbor,
    Binary,
}

impl MessagePayloadEncoding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Utf8 => "utf8",
            Self::Json => "json",
            Self::Cbor => "cbor",
            Self::Binary => "binary",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "utf8" => Some(Self::Utf8),
            "json" => Some(Self::Json),
            "cbor" => Some(Self::Cbor),
            "binary" => Some(Self::Binary),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessagePayloadCompression {
    None,
    Zstd,
}

impl MessagePayloadCompression {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Zstd => "zstd",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "none" => Some(Self::None),
            "zstd" => Some(Self::Zstd),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedMessagePayload {
    pub family: MessagePayloadFamily,
    pub encoding: MessagePayloadEncoding,
    pub compression: MessagePayloadCompression,
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
pub enum MessagePayloadCodecError {
    InvalidJson(serde_json::Error),
    InvalidCbor(serde_cbor::Error),
    InvalidUtf8(std::string::FromUtf8Error),
    UnsupportedEncoding {
        family: MessagePayloadFamily,
        encoding: MessagePayloadEncoding,
    },
    Compression(std::io::Error),
}

impl fmt::Display for MessagePayloadCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(error) => write!(f, "invalid json payload: {error}"),
            Self::InvalidCbor(error) => write!(f, "invalid cbor payload: {error}"),
            Self::InvalidUtf8(error) => write!(f, "invalid utf-8 payload: {error}"),
            Self::UnsupportedEncoding { family, encoding } => write!(
                f,
                "unsupported payload encoding {} for {}",
                encoding.as_str(),
                family.as_str()
            ),
            Self::Compression(error) => write!(f, "payload compression error: {error}"),
        }
    }
}

impl std::error::Error for MessagePayloadCodecError {}

pub fn encode_message_payload(
    payload: &MessagePayload,
) -> Result<EncodedMessagePayload, MessagePayloadCodecError> {
    let (family, encoding, bytes) = match payload {
        MessagePayload::Text { text } => (
            MessagePayloadFamily::Text,
            MessagePayloadEncoding::Utf8,
            text.as_bytes().to_vec(),
        ),
        MessagePayload::Json { value } => (
            MessagePayloadFamily::Json,
            MessagePayloadEncoding::Json,
            serde_json::to_vec(value).map_err(MessagePayloadCodecError::InvalidJson)?,
        ),
        MessagePayload::Binary { bytes } => (
            MessagePayloadFamily::Binary,
            MessagePayloadEncoding::Binary,
            bytes.clone(),
        ),
    };
    Ok(EncodedMessagePayload {
        family,
        encoding,
        compression: MessagePayloadCompression::None,
        bytes,
    })
}

pub fn encode_message_payload_with_encoding(
    payload: &MessagePayload,
    encoding: MessagePayloadEncoding,
    compression: MessagePayloadCompression,
) -> Result<EncodedMessagePayload, MessagePayloadCodecError> {
    let encoded = match (payload, encoding) {
        (MessagePayload::Text { text }, MessagePayloadEncoding::Utf8) => EncodedMessagePayload {
            family: MessagePayloadFamily::Text,
            encoding,
            compression,
            bytes: text.as_bytes().to_vec(),
        },
        (MessagePayload::Json { value }, MessagePayloadEncoding::Json) => EncodedMessagePayload {
            family: MessagePayloadFamily::Json,
            encoding,
            compression,
            bytes: serde_json::to_vec(value).map_err(MessagePayloadCodecError::InvalidJson)?,
        },
        (MessagePayload::Json { value }, MessagePayloadEncoding::Cbor) => EncodedMessagePayload {
            family: MessagePayloadFamily::Json,
            encoding,
            compression,
            bytes: serde_cbor::to_vec(value).map_err(MessagePayloadCodecError::InvalidCbor)?,
        },
        (MessagePayload::Binary { bytes }, MessagePayloadEncoding::Binary) => {
            EncodedMessagePayload {
                family: MessagePayloadFamily::Binary,
                encoding,
                compression,
                bytes: bytes.clone(),
            }
        }
        (payload, encoding) => {
            return Err(MessagePayloadCodecError::UnsupportedEncoding {
                family: match payload {
                    MessagePayload::Text { .. } => MessagePayloadFamily::Text,
                    MessagePayload::Json { .. } => MessagePayloadFamily::Json,
                    MessagePayload::Binary { .. } => MessagePayloadFamily::Binary,
                },
                encoding,
            });
        }
    };
    apply_compression(encoded)
}

pub fn decode_message_payload(
    family: MessagePayloadFamily,
    encoding: MessagePayloadEncoding,
    compression: MessagePayloadCompression,
    bytes: &[u8],
) -> Result<MessagePayload, MessagePayloadCodecError> {
    let bytes = decode_compression(compression, bytes)?;
    match (family, encoding) {
        (MessagePayloadFamily::Text, MessagePayloadEncoding::Utf8) => String::from_utf8(bytes)
            .map(|text| MessagePayload::Text { text })
            .map_err(MessagePayloadCodecError::InvalidUtf8),
        (MessagePayloadFamily::Json, MessagePayloadEncoding::Json) => {
            serde_json::from_slice(&bytes)
                .map(|value| MessagePayload::Json { value })
                .map_err(MessagePayloadCodecError::InvalidJson)
        }
        (MessagePayloadFamily::Json, MessagePayloadEncoding::Cbor) => {
            serde_cbor::from_slice(&bytes)
                .map(|value| MessagePayload::Json { value })
                .map_err(MessagePayloadCodecError::InvalidCbor)
        }
        (MessagePayloadFamily::Binary, MessagePayloadEncoding::Binary) => {
            Ok(MessagePayload::Binary { bytes })
        }
        (family, encoding) => {
            Err(MessagePayloadCodecError::UnsupportedEncoding { family, encoding })
        }
    }
}

fn apply_compression(
    encoded: EncodedMessagePayload,
) -> Result<EncodedMessagePayload, MessagePayloadCodecError> {
    let bytes = match encoded.compression {
        MessagePayloadCompression::None => encoded.bytes,
        MessagePayloadCompression::Zstd => zstd::encode_all(encoded.bytes.as_slice(), 0)
            .map_err(MessagePayloadCodecError::Compression)?,
    };
    Ok(EncodedMessagePayload { bytes, ..encoded })
}

fn decode_compression(
    compression: MessagePayloadCompression,
    bytes: &[u8],
) -> Result<Vec<u8>, MessagePayloadCodecError> {
    match compression {
        MessagePayloadCompression::None => Ok(bytes.to_vec()),
        MessagePayloadCompression::Zstd => {
            zstd::decode_all(bytes).map_err(MessagePayloadCodecError::Compression)
        }
    }
}


