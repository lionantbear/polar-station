//! Transport- and UI-independent Polar Station Inbox semantics.
//!
//! This crate intentionally stores only privacy-safe summaries, replies,
//! drafts, approvals, and delivery receipts. Raw terminal capture never enters
//! its public model.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

mod codex;
pub use codex::{
    CodexCompletion, CodexCompletionCursor, CodexCompletionOrigin, CodexCompletionStatus,
};

pub const RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1_000;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InboxMessageId(String);

impl InboxMessageId {
    pub fn new(value: impl Into<String>) -> Result<Self, InboxError> {
        let value = value.into();
        validate_text(&value, 1, 128)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalOrigin {
    pub workspace_id: String,
    pub tab_id: String,
    pub pane_id: String,
    pub execution_target_id: String,
    pub tmux_session_id: String,
    pub consumer_generation: u64,
    /// Opaque to Rust. Only the native client may interpret this token.
    pub native_recovery_token: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    Unknown,
    Working,
    ReviewReady,
    NeedsAttention,
    ApprovalRequired,
    DecisionRequired,
    PermissionRequired,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionSource {
    #[default]
    Unspecified,
    TmuxActivity,
    TmuxBell,
    PaneTitle,
    AgentHook,
    AiClassification,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionConfidence {
    Direct,
    #[default]
    Inferred,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    #[default]
    Insert,
    Send,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftStatus {
    None,
    Generating,
    Ready,
    Failed,
    Approved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    NotRequested,
    Restoring,
    Delivered,
    FailedBeforeWrite,
    DeliveryUnknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiInvocation {
    pub provider: String,
    pub model: Option<String>,
    pub duration_ms: u64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxMessage {
    pub id: InboxMessageId,
    pub origin: TerminalOrigin,
    pub session_label: String,
    pub attention: AttentionKind,
    #[serde(default)]
    pub source: AttentionSource,
    #[serde(default)]
    pub confidence: AttentionConfidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_event_id: Option<String>,
    pub summary: String,
    pub source_revision: u64,
    pub created_at_ms: u64,
    pub revision: u64,
    pub unread: bool,
    pub reply: Option<String>,
    pub draft: Option<String>,
    pub draft_status: DraftStatus,
    pub delivery_status: DeliveryStatus,
    pub ai_invocation: Option<AiInvocation>,
}

impl InboxMessage {
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn new_for_store(
        id: InboxMessageId,
        origin: TerminalOrigin,
        session_label: String,
        attention: AttentionKind,
        source: AttentionSource,
        confidence: AttentionConfidence,
        source_event_id: Option<String>,
        summary: String,
        source_revision: u64,
        created_at_ms: u64,
    ) -> Self {
        Self {
            id,
            origin,
            session_label,
            attention,
            source,
            confidence,
            source_event_id,
            summary,
            source_revision,
            created_at_ms,
            revision: 0,
            unread: true,
            reply: None,
            draft: None,
            draft_status: DraftStatus::None,
            delivery_status: DeliveryStatus::NotRequested,
            ai_invocation: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageInput {
    pub origin: TerminalOrigin,
    pub session_label: String,
    pub attention: AttentionKind,
    pub source: AttentionSource,
    pub confidence: AttentionConfidence,
    pub source_event_id: Option<String>,
    pub summary: String,
    pub source_revision: u64,
    pub occurred_at_ms: u64,
    pub ai_invocation: Option<AiInvocation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObserveOutcome {
    Created(InboxMessageId),
    Updated(InboxMessageId),
    Unchanged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryAuthorization {
    pub message_id: InboxMessageId,
    pub origin: TerminalOrigin,
    pub text: String,
    pub mode: DeliveryMode,
    pub approval_nonce: String,
}

#[derive(Default)]
pub struct Inbox {
    ordered: Vec<InboxMessageId>,
    messages: HashMap<InboxMessageId, InboxMessage>,
    last_edge: HashMap<(String, String), (AttentionKind, u64, InboxMessageId)>,
    source_events: HashMap<String, InboxMessageId>,
    consumed_nonces: HashSet<String>,
}

impl Inbox {
    pub fn restore(&mut self, message: InboxMessage) {
        if self.messages.contains_key(&message.id) {
            return;
        }
        let edge_key = (
            message.origin.execution_target_id.clone(),
            message.origin.tmux_session_id.clone(),
        );
        let replace_edge = self
            .last_edge
            .get(&edge_key)
            .is_none_or(|(_, occurred_at_ms, _)| message.created_at_ms > *occurred_at_ms);
        if replace_edge {
            self.last_edge.insert(
                edge_key,
                (message.attention, message.created_at_ms, message.id.clone()),
            );
        }
        if let Some(source_event_id) = &message.source_event_id {
            self.source_events
                .insert(source_event_id.clone(), message.id.clone());
        }
        self.ordered.push(message.id.clone());
        self.messages.insert(message.id.clone(), message);
    }
    pub fn observe(&mut self, input: MessageInput) -> Result<ObserveOutcome, InboxError> {
        validate_origin(&input.origin)?;
        validate_text(&input.session_label, 1, 128)?;
        validate_text(&input.summary, 1, 4_096)?;
        if let Some(source_event_id) = &input.source_event_id {
            validate_text(source_event_id, 1, 128)?;
            if let Some(id) = self.source_events.get(source_event_id).cloned() {
                return self.update_observed_message(id, input);
            }
        }
        let edge_key = (
            input.origin.execution_target_id.clone(),
            input.origin.tmux_session_id.clone(),
        );
        if let Some((_, occurred_at_ms, id)) = self.last_edge.get(&edge_key) {
            if *occurred_at_ms == input.occurred_at_ms {
                return self.update_observed_message(id.clone(), input);
            }
        }
        let id = InboxMessageId::new(format!(
            "{}-{}-{}",
            input.occurred_at_ms,
            input.source_revision,
            self.ordered.len() + 1
        ))?;
        let mut message = InboxMessage::new_for_store(
            id.clone(),
            input.origin,
            input.session_label,
            input.attention,
            input.source,
            input.confidence,
            input.source_event_id.clone(),
            input.summary,
            input.source_revision,
            input.occurred_at_ms,
        );
        message.ai_invocation = input.ai_invocation;
        self.ordered.push(id.clone());
        self.messages.insert(id.clone(), message);
        if let Some(source_event_id) = input.source_event_id {
            self.source_events.insert(source_event_id, id.clone());
        }
        self.last_edge.insert(
            edge_key,
            (input.attention, input.occurred_at_ms, id.clone()),
        );
        Ok(ObserveOutcome::Created(id))
    }

    fn update_observed_message(
        &mut self,
        id: InboxMessageId,
        input: MessageInput,
    ) -> Result<ObserveOutcome, InboxError> {
        let message = self
            .messages
            .get_mut(&id)
            .ok_or(InboxError::MessageNotFound)?;
        let replace_evidence = message.confidence != AttentionConfidence::Direct
            || input.confidence == AttentionConfidence::Direct;
        let next_attention = if replace_evidence {
            input.attention
        } else {
            message.attention
        };
        let next_source = if replace_evidence {
            input.source
        } else {
            message.source
        };
        let next_confidence = if replace_evidence {
            input.confidence
        } else {
            message.confidence
        };
        let next_source_event_id = if replace_evidence && input.source_event_id.is_some() {
            input.source_event_id.clone()
        } else {
            message.source_event_id.clone()
        };
        if message.summary == input.summary
            && message.source_revision == input.source_revision
            && message.ai_invocation == input.ai_invocation
            && message.attention == next_attention
            && message.source == next_source
            && message.confidence == next_confidence
            && message.source_event_id == next_source_event_id
        {
            return Ok(ObserveOutcome::Unchanged);
        }
        message.summary = input.summary;
        message.source_revision = input.source_revision;
        message.ai_invocation = input.ai_invocation;
        message.attention = next_attention;
        message.source = next_source;
        message.confidence = next_confidence;
        message.source_event_id = next_source_event_id.clone();
        message.revision = message.revision.saturating_add(1);

        let edge_key = (
            message.origin.execution_target_id.clone(),
            message.origin.tmux_session_id.clone(),
        );
        self.last_edge.insert(
            edge_key,
            (message.attention, input.occurred_at_ms, id.clone()),
        );
        if let Some(source_event_id) = next_source_event_id {
            self.source_events.insert(source_event_id, id.clone());
        }
        Ok(ObserveOutcome::Updated(id))
    }

    pub fn message(&self, id: &InboxMessageId) -> Option<&InboxMessage> {
        self.messages.get(id)
    }

    pub fn messages(&self) -> Vec<&InboxMessage> {
        self.ordered
            .iter()
            .rev()
            .filter_map(|id| self.messages.get(id))
            .collect()
    }

    pub fn reply(
        &mut self,
        id: &InboxMessageId,
        expected_revision: u64,
        reply: &str,
    ) -> Result<u64, InboxError> {
        validate_text(reply, 1, 8_192)?;
        let message = self.message_mut(id, expected_revision)?;
        message.reply = Some(reply.to_owned());
        message.draft = None;
        message.draft_status = DraftStatus::Generating;
        message.revision += 1;
        Ok(message.revision)
    }

    pub fn record_draft(
        &mut self,
        id: &InboxMessageId,
        expected_revision: u64,
        draft: &str,
        model: &str,
        duration_ms: u64,
    ) -> Result<u64, InboxError> {
        validate_text(draft, 1, 16_384)?;
        validate_text(model, 1, 128)?;
        let message = self.message_mut(id, expected_revision)?;
        message.draft = Some(draft.to_owned());
        message.draft_status = DraftStatus::Ready;
        message.ai_invocation = Some(AiInvocation {
            provider: "codex_app_server".into(),
            model: Some(model.into()),
            duration_ms,
            error: None,
        });
        message.revision += 1;
        Ok(message.revision)
    }

    pub fn edit_draft(
        &mut self,
        id: &InboxMessageId,
        expected_revision: u64,
        draft: &str,
    ) -> Result<u64, InboxError> {
        validate_text(draft, 1, 16_384)?;
        let message = self.message_mut(id, expected_revision)?;
        message.draft = Some(draft.to_owned());
        message.draft_status = DraftStatus::Ready;
        message.revision += 1;
        Ok(message.revision)
    }

    pub fn fail_draft(
        &mut self,
        id: &InboxMessageId,
        expected_revision: u64,
        error: &str,
        duration_ms: u64,
    ) -> Result<u64, InboxError> {
        validate_text(error, 1, 512)?;
        let message = self.message_mut(id, expected_revision)?;
        message.draft_status = DraftStatus::Failed;
        message.ai_invocation = Some(AiInvocation {
            provider: "codex_app_server".into(),
            model: None,
            duration_ms,
            error: Some(error.to_owned()),
        });
        message.revision += 1;
        Ok(message.revision)
    }

    pub fn acknowledge(
        &mut self,
        id: &InboxMessageId,
        expected_revision: u64,
    ) -> Result<u64, InboxError> {
        let message = self.message_mut(id, expected_revision)?;
        message.unread = false;
        message.revision += 1;
        Ok(message.revision)
    }

    pub fn approve(
        &mut self,
        id: &InboxMessageId,
        expected_revision: u64,
        mode: DeliveryMode,
        nonce: &str,
    ) -> Result<DeliveryAuthorization, InboxError> {
        validate_text(nonce, 1, 128)?;
        if self.consumed_nonces.contains(nonce) {
            return Err(InboxError::ApprovalAlreadyConsumed);
        }
        let message = self.message_mut(id, expected_revision)?;
        let text = message.draft.clone().ok_or(InboxError::DraftNotReady)?;
        if message.draft_status != DraftStatus::Ready {
            return Err(InboxError::DraftNotReady);
        }
        message.draft_status = DraftStatus::Approved;
        message.revision += 1;
        let authorization = DeliveryAuthorization {
            message_id: id.clone(),
            origin: message.origin.clone(),
            text,
            mode,
            approval_nonce: nonce.to_owned(),
        };
        self.consumed_nonces.insert(nonce.to_owned());
        Ok(authorization)
    }

    pub fn record_delivery(
        &mut self,
        id: &InboxMessageId,
        expected_revision: u64,
        status: DeliveryStatus,
    ) -> Result<u64, InboxError> {
        if matches!(status, DeliveryStatus::NotRequested) {
            return Err(InboxError::InvalidDeliveryTransition);
        }
        let message = self.message_mut(id, expected_revision)?;
        if message.draft_status != DraftStatus::Approved {
            return Err(InboxError::DraftNotReady);
        }
        message.delivery_status = status;
        message.unread = false;
        message.revision += 1;
        Ok(message.revision)
    }

    fn message_mut(
        &mut self,
        id: &InboxMessageId,
        expected_revision: u64,
    ) -> Result<&mut InboxMessage, InboxError> {
        let message = self
            .messages
            .get_mut(id)
            .ok_or(InboxError::MessageNotFound)?;
        if message.revision != expected_revision {
            return Err(InboxError::RevisionConflict {
                expected: expected_revision,
                actual: message.revision,
            });
        }
        Ok(message)
    }
}

fn validate_origin(value: &TerminalOrigin) -> Result<(), InboxError> {
    for field in [
        &value.workspace_id,
        &value.tab_id,
        &value.pane_id,
        &value.execution_target_id,
        &value.tmux_session_id,
        &value.native_recovery_token,
    ] {
        validate_text(field, 1, 512)?;
    }
    Ok(())
}

fn validate_text(value: &str, minimum: usize, maximum: usize) -> Result<(), InboxError> {
    if value.len() < minimum
        || value.len() > maximum
        || value != value.trim()
        || value.chars().any(|c| c == '\0')
    {
        return Err(InboxError::InvalidText);
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InboxError {
    #[error("Inbox text is empty, malformed, or too large")]
    InvalidText,
    #[error("Inbox delivery state transition is invalid")]
    InvalidDeliveryTransition,
    #[error("Inbox message was not found")]
    MessageNotFound,
    #[error("Inbox revision conflict: expected {expected}, actual {actual}")]
    RevisionConflict { expected: u64, actual: u64 },
    #[error("AI draft is not ready")]
    DraftNotReady,
    #[error("approval was already consumed")]
    ApprovalAlreadyConsumed,
}


