//! Completion facts need an execution origin, never a fabricated desktop Pane.
use crate::{validate_text, InboxError, InboxMessageId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexCompletionOrigin {
    pub owner_host_id: String,
    pub endpoint_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub binding_id: String,
    pub binding_revision: u64,
    pub execution_target_id: String,
    /// Optional terminal presentation; Codex origin and replies do not require tmux.
    pub tmux_session_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexCompletionStatus {
    Completed,
    Failed,
    Interrupted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexCompletion {
    pub id: InboxMessageId,
    pub origin: CodexCompletionOrigin,
    pub status: CodexCompletionStatus,
    pub summary: String,
    pub created_at_ms: u64,
}

impl CodexCompletion {
    pub fn validate(&self) -> Result<(), InboxError> {
        validate_text(self.id.as_str(), 1, 128)?;
        let o = &self.origin;
        for value in [
            &o.owner_host_id,
            &o.endpoint_id,
            &o.thread_id,
            &o.turn_id,
            &o.binding_id,
            &o.execution_target_id,
        ] {
            validate_text(value, 1, 256)?;
            if value.chars().any(char::is_control) {
                return Err(InboxError::InvalidText);
            }
        }
        if let Some(session) = &o.tmux_session_id {
            validate_text(session, 1, 256)?;
            if session.chars().any(char::is_control) {
                return Err(InboxError::InvalidText);
            }
        }
        if o.binding_revision == 0 || self.created_at_ms > i64::MAX as u64 {
            return Err(InboxError::InvalidText);
        }
        validate_text(&self.summary, 1, 4096)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexCompletionCursor {
    pub created_at_ms: u64,
    pub id: InboxMessageId,
}
impl CodexCompletionCursor {
    pub fn validate(&self) -> Result<(), InboxError> {
        validate_text(self.id.as_str(), 1, 128)?;
        if self.created_at_ms > i64::MAX as u64 {
            return Err(InboxError::InvalidText);
        }
        Ok(())
    }
}
