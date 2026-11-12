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
    ResolveFileCandidates {
        origin: polar_tmux_resources::FileOrigin,
        candidates: Vec<polar_tmux_resources::FileReference>,
    },
    ResolveFile {
        execution_target_id: String,
        path: String,
        cwd: Option<String>,
        /// Authoritative source session; never infer a harness cwd from the UI.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_session: Option<crate::SessionKeyV1>,
        /// Opt in to the additive LocalFile result; old clients keep v1 results.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        client_supports_local_file: bool,
    },
    GetLogTail {
        resource_id: String,
    },
    Apply {
        catalog: ResourceCatalog,
    },
    Restart {
        resource_id: String,
    },
    OpenMirror {
        resource_id: String,
    },
}

impl ResourcesCommandV1 {
    pub fn is_mutation(&self) -> bool {
        !matches!(
            self,
            Self::GetProjection | Self::GetLogTail { .. } | Self::ResolveFile { .. } | Self::ResolveFileCandidates { .. }
        )
    }

    fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::ResolveFileCandidates { origin, candidates } => {
                validate_stable_identifier(&origin.execution_target_id, "execution target")?;
                if candidates.is_empty() || candidates.len() > polar_tmux_resources::MAX_FILE_CANDIDATES
                    || candidates.iter().map(|r| r.path.len()).sum::<usize>() > polar_tmux_resources::MAX_FILE_CANDIDATE_BYTES
                    || origin.terminal_session_id.as_ref().is_some_and(|s| s.is_empty() || s.len() > 256 || s.chars().any(char::is_control))
                    || origin.cwd.as_ref().is_some_and(|s| s.len() > 16384 || s.chars().any(char::is_control))
                    || candidates.iter().any(|r| r.path.is_empty() || r.path.len() > 16384 || r.path.chars().any(char::is_control)) {
                    return Err(ProtocolValidationError::InvalidResourcesCommand);
                }
                Ok(())
            }
            Self::ResolveFile {
                execution_target_id,
                path,
                cwd,
                source_session,
                ..
            } => {
                if let Some(session) = source_session {
                    session.validate()?;
                } else {
                    validate_stable_identifier(execution_target_id, "execution target")?;
                }
                if path.is_empty()
                    || path.len() > 16384
                    || path.chars().any(char::is_control)
                    || cwd
                        .as_ref()
                        .is_some_and(|v| v.len() > 16384 || v.chars().any(char::is_control))
                {
                    return Err(ProtocolValidationError::InvalidResourcesCommand);
                }
                Ok(())
            }
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
            ResourcesResultV1::FileAccess { .. } => Ok(()),
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
    FileAccess {
        access: polar_tmux_resources::FileAccess,
    },
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




