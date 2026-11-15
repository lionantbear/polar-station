use serde::{Deserialize, Serialize};

use crate::{validate_stable_identifier, ProtocolError, ProtocolValidationError};

pub const INBOX_API_VERSION: u32 = 1;

pub use polar_station_inbox::{
    AttentionConfidence as InboxAttentionConfidenceV1, AttentionSource as InboxAttentionSourceV1,
    DeliveryMode as DeliveryModeV1,
};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InboxMessageIdV1(String);

impl InboxMessageIdV1 {
    pub fn new(value: impl Into<String>) -> Result<Self, ProtocolValidationError> {
        let value = value.into();
        validate_stable_identifier(&value, "Inbox message ID")?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalOriginHandleV1 {
    pub workspace_id: String,
    pub tab_id: String,
    pub pane_id: String,
    pub execution_target_id: String,
    pub tmux_session_id: String,
    pub consumer_generation: u64,
    pub native_recovery_token: String,
}

impl TerminalOriginHandleV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        for (value, name) in [
            (&self.workspace_id, "Inbox Workspace ID"),
            (&self.tab_id, "Inbox Tab ID"),
            (&self.pane_id, "Inbox Pane ID"),
            (&self.execution_target_id, "Inbox target ID"),
            (&self.tmux_session_id, "Inbox tmux Session ID"),
            (&self.native_recovery_token, "Inbox native recovery token"),
        ] {
            validate_stable_identifier(value, name)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxDraftStatusV1 {
    None,
    Generating,
    Ready,
    Failed,
    Approved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryReceiptStateV1 {
    Restoring,
    Delivered,
    FailedBeforeWrite,
    DeliveryUnknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiInvocationProjectionV1 {
    pub provider: String,
    pub model: Option<String>,
    pub duration_ms: u64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxMessageProjectionV1 {
    pub id: InboxMessageIdV1,
    pub origin: TerminalOriginHandleV1,
    pub session_label: String,
    pub attention: String,
    #[serde(default)]
    pub source: InboxAttentionSourceV1,
    #[serde(default)]
    pub confidence: InboxAttentionConfidenceV1,
    pub summary: String,
    pub created_at_ms: u64,
    pub revision: u64,
    pub unread: bool,
    pub reply: Option<String>,
    pub draft: Option<String>,
    pub draft_status: InboxDraftStatusV1,
    pub stale_context: bool,
    pub ai_invocation: Option<AiInvocationProjectionV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxPageV1 {
    pub messages: Vec<InboxMessageProjectionV1>,
    pub next_before_created_at_ms: Option<u64>,
    pub unread_count: u32,
    #[serde(default)]
    pub overview: InboxOverviewV1,
}

/// Read projection over existing monitor and completion stores. A row never
/// grants authority to send: native actions reopen its exact original identity.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxOverviewV1 {
    pub activity: Vec<InboxOverviewRowV1>,
    pub records: Vec<InboxOverviewRowV1>,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxOverviewRowV1 {
    pub id: String,
    pub source: String,
    pub execution_target_id: String,
    pub title: String,
    pub task_status: String,
    pub task_label: String,
    pub observation_label: String,
    pub stale: bool,
    pub summary: String,
    pub created_at_ms: u64,
    pub session: Option<crate::SessionKeyV1>,
    pub terminal_message_id: Option<InboxMessageIdV1>,
    pub codex_owner_id: Option<String>,
    pub codex_binding_id: Option<String>,
    pub deliveries: Vec<crate::CodexDeliveryProjectionV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxApiV1 {
    InboxV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InboxCommandV1 {
    /// Native-origin metadata is opaque to the Host. Attention facts are
    /// looked up by Session identity, never supplied by the renderer.
    ObserveAttention { origin: TerminalOriginHandleV1 },
    GetPage {
        before_created_at_ms: Option<u64>,
        limit: u16,
    },
    Reply {
        message_id: InboxMessageIdV1,
        expected_revision: u64,
        reply: String,
    },
    Regenerate {
        message_id: InboxMessageIdV1,
        expected_revision: u64,
    },
    EditDraft {
        message_id: InboxMessageIdV1,
        expected_revision: u64,
        draft: String,
    },
    Approve {
        message_id: InboxMessageIdV1,
        expected_revision: u64,
        delivery_mode: DeliveryModeV1,
        approval_nonce: String,
    },
    RecordDelivery {
        message_id: InboxMessageIdV1,
        expected_revision: u64,
        state: DeliveryReceiptStateV1,
    },
    Acknowledge {
        message_id: InboxMessageIdV1,
        expected_revision: u64,
    },
}

impl InboxCommandV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::ObserveAttention { origin } => origin.validate(),
            Self::GetPage { limit, .. } if !(1..=200).contains(limit) => Err(
                ProtocolValidationError::InvalidIdentifier("Inbox page limit"),
            ),
            Self::GetPage { .. } => Ok(()),
            Self::Reply {
                message_id, reply, ..
            } => {
                validate_message_id(message_id)?;
                validate_body(reply, "Inbox reply", 8_192)
            }
            Self::EditDraft {
                message_id, draft, ..
            } => {
                validate_message_id(message_id)?;
                validate_body(draft, "Inbox draft", 16_384)
            }
            Self::Approve {
                message_id,
                approval_nonce,
                ..
            } => {
                validate_message_id(message_id)?;
                validate_stable_identifier(approval_nonce, "Inbox approval nonce")
            }
            Self::Regenerate { message_id, .. }
            | Self::RecordDelivery { message_id, .. }
            | Self::Acknowledge { message_id, .. } => validate_message_id(message_id),
        }
    }
}

fn validate_message_id(id: &InboxMessageIdV1) -> Result<(), ProtocolValidationError> {
    validate_stable_identifier(id.as_str(), "Inbox message ID")
}

fn validate_body(
    value: &str,
    field: &'static str,
    max: usize,
) -> Result<(), ProtocolValidationError> {
    if value.is_empty() || value.len() > max || value != value.trim() || value.contains('\0') {
        Err(ProtocolValidationError::InvalidIdentifier(field))
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxRequestEnvelopeV1 {
    pub protocol_version: u32,
    pub api: InboxApiV1,
    pub request_id: String,
    pub command: InboxCommandV1,
}

impl InboxRequestEnvelopeV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.protocol_version != crate::PROTOCOL_VERSION || self.api != InboxApiV1::InboxV1 {
            return Err(ProtocolValidationError::InvalidIdentifier(
                "Inbox API version",
            ));
        }
        validate_stable_identifier(&self.request_id, "Inbox request ID")?;
        self.command.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InboxResultV1 {
    Page {
        page: InboxPageV1,
    },
    Message {
        message: InboxMessageProjectionV1,
    },
    DeliveryRequested {
        message_id: InboxMessageIdV1,
        origin: TerminalOriginHandleV1,
        text: String,
        delivery_mode: DeliveryModeV1,
        approval_nonce: String,
    },
    DeliveryReceipt {
        message_id: InboxMessageIdV1,
        state: DeliveryReceiptStateV1,
        error: Option<ProtocolError>,
    },
    Error {
        error: ProtocolError,
        message: Option<InboxMessageProjectionV1>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxResponseEnvelopeV1 {
    pub protocol_version: u32,
    pub api: InboxApiV1,
    pub request_id: String,
    pub event_revision: u64,
    pub result: InboxResultV1,
}
