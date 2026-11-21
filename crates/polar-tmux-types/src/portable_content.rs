//! Provenance carried by a saved terminal binding. Never an authorization grant.
use crate::{validate_stable_identifier, ProtocolValidationError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostRefV1 {
    pub catalog_id: String,
    pub target_id: String,
}

impl HostRefV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_stable_identifier(&self.catalog_id, "Host catalog")?;
        validate_stable_identifier(&self.target_id, "Host target")
    }
}

/// Public digest of the existing tmux server-generation observation. Neither
/// socket paths nor executable attach commands are part of this reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedTmuxSessionV1 {
    pub server_generation: String,
    pub session_id: String,
    pub created_unix_seconds: i64,
}

/// Live attachment identity scoped to one selected Host. A changed transport
/// alone does not change this identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalBindingIdentityV1 {
    pub execution_target: crate::ExecutionTarget,
    pub target_revision: u64,
    pub session: ObservedTmuxSessionV1,
}

impl ObservedTmuxSessionV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.server_generation.len() != 64
            || !self
                .server_generation
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
            || self.created_unix_seconds < 0
            || !self.session_id.strip_prefix('$').is_some_and(|id| {
                !id.is_empty()
                    && id.bytes().all(|b| b.is_ascii_digit())
                    && id.parse::<u64>().is_ok()
            })
        {
            return Err(ProtocolValidationError::InvalidIdentifier(
                "original tmux Session",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalSourceV1 {
    pub host: HostRefV1,
    /// Absent when export cannot establish the original live process. Keeping
    /// the reference is allowed; attaching to a guessed replacement is not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<ObservedTmuxSessionV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_agent_id: Option<String>,
}

impl TerminalSourceV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        self.host.validate()?;
        if let Some(session) = &self.session {
            session.validate()?;
        }
        if let Some(agent) = &self.original_agent_id {
            validate_stable_identifier(agent, "original agent Session")?;
        }
        Ok(())
    }
}
