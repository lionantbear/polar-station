//! Resources v1 transport envelopes.
//!
//! The catalog and projection remain owned by `polar-tmux-resources`; this
//! module only freezes their request/response wire composition.

use polar_tmux_resources::{
    ResourceCatalog, ResourcesControlProjection, RESOURCE_CATALOG_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};

use crate::{validate_stable_identifier, ProtocolError, ProtocolValidationError, PROTOCOL_VERSION};

pub const RESOURCES_API_VERSION: u16 = 1;
pub const RESOURCE_LOG_MAX_EVENTS: usize = 200;
pub const RESOURCE_LOG_MAX_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceLogLevelV1 {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLogEventV1 {
    pub timestamp_ms: u64,
    pub level: ResourceLogLevelV1,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLogTailV1 {
    pub resource_id: String,
    pub events: Vec<ResourceLogEventV1>,
    pub truncated: bool,
}

impl ResourceLogTailV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_stable_identifier(&self.resource_id, "Resource ID")?;
        if self.events.len() > RESOURCE_LOG_MAX_EVENTS
            || self
                .events
                .iter()
                .map(|event| event.message.len())
                .sum::<usize>()
                > RESOURCE_LOG_MAX_BYTES
            || self.events.iter().any(|event| {
                event.message.is_empty()
                    || event.message.len() > RESOURCE_LOG_MAX_BYTES
                    || event.message.chars().any(char::is_control)
            })
        {
            return Err(ProtocolValidationError::InvalidResourcesCommand);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourcesApiV1 {
    ResourcesV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourcesRequestEnvelopeV1 {
    pub protocol_version: u32,
    pub api: ResourcesApiV1,
    pub request_id: String,
    pub expected_revision: Option<u64>,
    pub command: ResourcesCommandV1,
}

impl ResourcesRequestEnvelopeV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ProtocolValidationError::UnsupportedProtocolVersion(
                self.protocol_version,
            ));
        }
        validate_stable_identifier(&self.request_id, "request ID")?;
        if self.command.is_mutation() != self.expected_revision.is_some() {
            return Err(ProtocolValidationError::MissingExpectedRevision);
        }
        self.command.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResourcesCommandV1 {
    GetProjection,
    GetLogTail { resource_id: String },
    Apply { catalog: ResourceCatalog },
    Restart { resource_id: String },
    OpenMirror { resource_id: String },
}

impl ResourcesCommandV1 {
    pub fn is_mutation(&self) -> bool {
        !matches!(self, Self::GetProjection | Self::GetLogTail { .. })
    }

    fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::GetProjection => Ok(()),
            Self::Apply { catalog } => {
                if catalog.schema_version != RESOURCE_CATALOG_SCHEMA_VERSION {
                    return Err(ProtocolValidationError::InvalidResourcesCommand);
                }
                Ok(())
            }
            Self::GetLogTail { resource_id }
            | Self::Restart { resource_id }
            | Self::OpenMirror { resource_id } => {
                validate_stable_identifier(resource_id, "Resource ID")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourcesResponseEnvelopeV1 {
    pub protocol_version: u32,
    pub api: ResourcesApiV1,
    pub request_id: String,
    pub revision: u64,
    pub result: ResourcesResultV1,
}

impl ResourcesResponseEnvelopeV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ProtocolValidationError::UnsupportedProtocolVersion(
                self.protocol_version,
            ));
        }
        validate_stable_identifier(&self.request_id, "request ID")?;
        match &self.result {
            ResourcesResultV1::Projection { projection }
            | ResourcesResultV1::CommandAccepted { projection } => {
                validate_projection(self.revision, projection)
            }
            ResourcesResultV1::LogTail { tail } => tail.validate(),
            ResourcesResultV1::Error { error, projection } => {
                error.validate_sanitized()?;
                if let Some(projection) = projection {
                    validate_projection(self.revision, projection)?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResourcesResultV1 {
    Projection {
        projection: ResourcesControlProjection,
    },
    CommandAccepted {
        projection: ResourcesControlProjection,
    },
    LogTail {
        tail: ResourceLogTailV1,
    },
    Error {
        error: ProtocolError,
        projection: Option<ResourcesControlProjection>,
    },
}

fn validate_projection(
    revision: u64,
    projection: &ResourcesControlProjection,
) -> Result<(), ProtocolValidationError> {
    if projection.schema_version != RESOURCE_CATALOG_SCHEMA_VERSION
        || projection.revision != revision
        || projection.catalog.revision != revision
    {
        return Err(ProtocolValidationError::InvalidResourcesCommand);
    }
    Ok(())
}
