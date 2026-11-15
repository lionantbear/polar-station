//! Bounded optional local-IPC extensions. No Polar SDK or business types.
//!
//! Offers live in optional *outer* Register/Registered fields: older peers
//! ignore those fields and never receive an unnegotiated new frame. Do not
//! add fields to the legacy deny_unknown_fields HostControlVersions object.

use std::collections::BTreeSet;
use std::io::{self, Write};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{validate_stable_identifier, ProtocolError, ProtocolValidationError};

pub const LOCAL_EXTENSION_VERSION: u16 = 1;
pub const POLAR_ACCESS_EXTENSION: &str = "polar_access";
pub const MAX_LOCAL_EXTENSIONS: usize = 16;
pub const MAX_EXTENSION_PAYLOAD_BYTES: usize = 512 * 1024;
pub const MAX_EXTENSION_IN_FLIGHT: usize = 16;
pub const MAX_EXTENSION_TIMEOUT_MS: u32 = 30_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalExtensionOfferV1 {
    pub name: String,
    pub version: u16,
    pub max_payload_bytes: u32,
}

impl LocalExtensionOfferV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_extension_name(&self.name)?;
        if self.version == 0 {
            return Err(invalid("extension version must be nonzero"));
        }
        if self.max_payload_bytes == 0
            || self.max_payload_bytes as usize > MAX_EXTENSION_PAYLOAD_BYTES
        {
            return Err(invalid(
                "extension payload limit is outside the supported bound",
            ));
        }
        Ok(())
    }

    pub fn permits(&self, name: &str, version: u16) -> bool {
        self.name == name && self.version == version
    }
}

pub fn validate_extension_offers(
    offers: &[LocalExtensionOfferV1],
) -> Result<(), ProtocolValidationError> {
    if offers.len() > MAX_LOCAL_EXTENSIONS {
        return Err(invalid("too many extension offers"));
    }
    let mut names = BTreeSet::new();
    for offer in offers {
        offer.validate()?;
        if !names.insert(offer.name.as_str()) {
            return Err(invalid("duplicate extension offer"));
        }
    }
    Ok(())
}

/// Exact version intersection. Unknown names/versions are not protocol errors.
/// Advertising transport support never authenticates a Polar node or target.
pub fn negotiate_local_extensions(
    offered: &[LocalExtensionOfferV1],
    supported: &[LocalExtensionOfferV1],
) -> Result<Vec<LocalExtensionOfferV1>, ProtocolValidationError> {
    validate_extension_offers(offered)?;
    validate_extension_offers(supported)?;
    let mut accepted = Vec::new();
    for offer in offered {
        if let Some(support) = supported
            .iter()
            .find(|s| s.permits(&offer.name, offer.version))
        {
            accepted.push(LocalExtensionOfferV1 {
                name: offer.name.clone(),
                version: offer.version,
                max_payload_bytes: offer.max_payload_bytes.min(support.max_payload_bytes),
            });
        }
    }
    Ok(accepted)
}

/// A Host cannot grant an unoffered version or a larger payload budget.
pub fn validate_extension_ack(
    offered: &[LocalExtensionOfferV1],
    accepted: &[LocalExtensionOfferV1],
) -> Result<(), ProtocolValidationError> {
    validate_extension_offers(offered)?;
    validate_extension_offers(accepted)?;
    for ack in accepted {
        if !offered.iter().any(|o| {
            o.permits(&ack.name, ack.version) && ack.max_payload_bytes <= o.max_payload_bytes
        }) {
            return Err(invalid(
                "Host acknowledged an unoffered extension or payload limit",
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalExtensionRequestV1 {
    pub extension: String,
    pub version: u16,
    pub request_id: String,
    pub timeout_ms: u32,
    pub payload: Value,
}

impl LocalExtensionRequestV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        self.validate_with_limit(MAX_EXTENSION_PAYLOAD_BYTES)
    }

    pub fn validate_with_limit(&self, limit: usize) -> Result<(), ProtocolValidationError> {
        validate_extension_name(&self.extension)?;
        validate_stable_identifier(&self.request_id, "extension request ID")?;
        if self.version == 0 {
            return Err(invalid("extension version must be nonzero"));
        }
        if self.timeout_ms == 0 || self.timeout_ms > MAX_EXTENSION_TIMEOUT_MS {
            return Err(invalid("extension deadline is outside the supported bound"));
        }
        validate_extension_payload(&self.payload, limit)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalExtensionCancelV1 {
    pub extension: String,
    pub version: u16,
    pub request_id: String,
}

impl LocalExtensionCancelV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_extension_name(&self.extension)?;
        validate_stable_identifier(&self.request_id, "extension request ID")?;
        if self.version == 0 {
            return Err(invalid("extension version must be nonzero"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalExtensionResponseV1 {
    pub extension: String,
    pub version: u16,
    pub request_id: String,
    pub result: LocalExtensionResultV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LocalExtensionResultV1 {
    Success {
        payload: Value,
    },
    Unsupported {},
    Rejected {
        error: ProtocolError,
    },
    /// Cancelling delivery does not roll back an already committed mutation.
    Cancelled {},
}

impl LocalExtensionResponseV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        self.validate_with_limit(MAX_EXTENSION_PAYLOAD_BYTES)
    }

    pub fn validate_with_limit(&self, limit: usize) -> Result<(), ProtocolValidationError> {
        validate_extension_name(&self.extension)?;
        validate_stable_identifier(&self.request_id, "extension request ID")?;
        if self.version == 0 {
            return Err(invalid("extension version must be nonzero"));
        }
        match &self.result {
            LocalExtensionResultV1::Success { payload } => {
                validate_extension_payload(payload, limit)
            }
            LocalExtensionResultV1::Rejected { error } => error.validate_sanitized(),
            LocalExtensionResultV1::Unsupported {} | LocalExtensionResultV1::Cancelled {} => Ok(()),
        }
    }

    pub fn for_request(request: &LocalExtensionRequestV1, result: LocalExtensionResultV1) -> Self {
        Self {
            extension: request.extension.clone(),
            version: request.version,
            request_id: request.request_id.clone(),
            result,
        }
    }
}

/// Count serialized bytes without allocating another payload-sized Vec. The
/// enclosing shared codec already rejects >1MiB frame lengths before parsing.
/// This tighter budget includes escaping, not just the UTF-8 content length.
pub fn validate_extension_payload(
    payload: &Value,
    limit: usize,
) -> Result<(), ProtocolValidationError> {
    if limit == 0 || limit > MAX_EXTENSION_PAYLOAD_BYTES {
        return Err(invalid("invalid extension budget"));
    }
    struct Budget {
        remaining: usize,
    }
    impl Write for Budget {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.remaining {
                return Err(io::Error::other("extension payload limit"));
            }
            self.remaining -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Budget { remaining: limit }, payload)
        .map_err(|_| invalid("extension JSON payload exceeds negotiated limit"))
}

fn validate_extension_name(name: &str) -> Result<(), ProtocolValidationError> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b))
    {
        return Err(invalid("invalid extension name"));
    }
    Ok(())
}

fn invalid(message: &'static str) -> ProtocolValidationError {
    ProtocolValidationError::InvalidIdentifier(message)
}

/// Asynchronous data sent only after a client explicitly subscribes through an
/// already negotiated extension. This lane is separate from control responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalExtensionEventV1 {
    pub extension: String,
    pub version: u16,
    pub subscription_id: String,
    pub sequence: u64,
    pub payload: Value,
}
impl LocalExtensionEventV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        self.validate_with_limit(MAX_EXTENSION_PAYLOAD_BYTES)
    }
    pub fn validate_with_limit(&self, limit: usize) -> Result<(), ProtocolValidationError> {
        validate_extension_name(&self.extension)?;
        validate_stable_identifier(&self.subscription_id, "extension subscription ID")?;
        if self.version == 0 || self.sequence == 0 {
            return Err(invalid("invalid event sequence/version"));
        }
        validate_extension_payload(&self.payload, limit)
    }
}
