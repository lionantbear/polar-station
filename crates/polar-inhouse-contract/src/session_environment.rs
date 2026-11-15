//! Shared association and invocation contracts for initial session preparation.
//! These values are supplied by trusted runtime code, never from model arguments.

use std::num::NonZeroU64;

use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

use crate::InhouseBackendTurnRef;

/// Canonical identity from a successfully bound tool, shared with prompt preparation.
/// Family membership comes from that tool's declaration; Gear IDs are actual
/// contributions in its resolved Loadout, not a list of installed equipment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundPromptTool {
    pub tool_id: String,
    /// Current model-call name from the exact bound descriptor, never inferred from the ID.
    pub ai_name: String,
    pub family_ids: Vec<String>,
    pub gear_ids: Vec<String>,
}

/// Actual caller, not the optional Polar addressed by a human command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolActor {
    Human {
        node_id: String,
    },
    InhousePolar {
        inhouse_polar_id: String,
        backend_profile_id: String,
    },
}

impl ToolActor {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Human { node_id } => ensure!(
                !node_id.trim().is_empty(),
                "tool caller requires human node_id"
            ),
            Self::InhousePolar {
                inhouse_polar_id,
                backend_profile_id,
            } => {
                ensure!(
                    !inhouse_polar_id.trim().is_empty(),
                    "tool caller requires inhouse_polar_id"
                );
                ensure!(
                    !backend_profile_id.trim().is_empty(),
                    "tool caller requires backend_profile_id"
                );
            }
        }
        Ok(())
    }

    pub fn inhouse_polar_id(&self) -> Option<&str> {
        match self {
            Self::InhousePolar {
                inhouse_polar_id, ..
            } => Some(inhouse_polar_id),
            Self::Human { .. } => None,
        }
    }
}

/// Stable tool binding scope, also usable without an LLM session.
/// In particular it contains no first request message ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSessionScope {
    pub owner_id: String,
    pub actor: ToolActor,
    pub role_id: Option<String>,
    pub channel_id: Option<String>,
    pub thread_id: String,
}

impl ToolSessionScope {
    pub fn validate(&self) -> Result<()> {
        self.actor.validate()?;
        ensure!(
            self.actor.inhouse_polar_id().is_some() || self.role_id.is_none(),
            "human tool binding cannot borrow an inhouse role"
        );
        for (field, value) in [
            ("owner_id", self.owner_id.as_str()),
            ("thread_id", self.thread_id.as_str()),
        ] {
            ensure!(
                !value.trim().is_empty(),
                "tool session scope requires {field}"
            );
        }
        for (field, value) in [("role_id", &self.role_id), ("channel_id", &self.channel_id)] {
            if let Some(value) = value {
                ensure!(
                    !value.trim().is_empty(),
                    "tool session scope has empty {field}"
                );
            }
        }
        Ok(())
    }
}

/// Trusted Human or Agent request identity, supplied anew on each invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCallContext {
    pub owner_id: String,
    pub thread_id: String,
    pub actor: ToolActor,
    pub request_message_id: String,
    pub request_sender_id: Option<String>,
}

impl ToolCallContext {
    pub fn from_inhouse(
        turn_ref: InhouseBackendTurnRef,
        request_sender_id: Option<String>,
    ) -> Self {
        Self {
            owner_id: turn_ref.owner_id,
            thread_id: turn_ref.thread_id,
            actor: ToolActor::InhousePolar {
                inhouse_polar_id: turn_ref.inhouse_polar_id,
                backend_profile_id: turn_ref.backend_profile_id,
            },
            request_message_id: turn_ref.request_message_id,
            request_sender_id,
        }
    }

    pub fn validate_for(&self, scope: &ToolSessionScope) -> Result<()> {
        scope.validate()?;
        ensure!(
            self.owner_id == scope.owner_id
                && self.thread_id == scope.thread_id
                && self.actor == scope.actor,
            "tool call context does not match the bound session scope"
        );
        ensure!(
            !self.request_message_id.trim().is_empty(),
            "tool call requires current request_message_id"
        );
        if let ToolActor::Human { node_id } = &self.actor {
            ensure!(
                self.request_sender_id.as_deref() == Some(node_id.as_str()),
                "human tool caller does not match source message sender"
            );
        }
        if let Some(sender) = &self.request_sender_id {
            ensure!(
                !sender.trim().is_empty(),
                "tool call has empty request_sender_id"
            );
        }
        Ok(())
    }
}

/// Joins prepared tools, prompt blocks and initial session/turn inputs.
/// The preparation service owns computing the digest; this type is not a handler snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionEnvironmentRef {
    pub revision: NonZeroU64,
    pub digest: String,
}

impl SessionEnvironmentRef {
    pub fn validate(&self) -> Result<()> {
        let hex = self.digest.strip_prefix("sha256:").unwrap_or("");
        ensure!(
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
            "environment digest must be sha256 followed by 64 lowercase hex digits"
        );
        Ok(())
    }
}
