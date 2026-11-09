use polar_identity::{NodeId, UnixTimestamp};
use polar_payload_codec::{
    decode_message_payload, MessagePayloadCodecError, MessagePayloadCompression,
    MessagePayloadEncoding, MessagePayloadFamily,
};
use polar_protocol::MessagePayload;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechDecisionMode {
    Delegated,
    Rewritten,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechRequestStatus {
    Pending,
    Approved,
    Rejected,
    Sent,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechRequestRecord {
    pub request_id: String,
    pub source_channel_id: String,
    pub source_thread_id: String,
    pub source_message_id: String,
    pub requested_by_inhouse_polar_id: Option<String>,
    pub requested_by_human_label: Option<String>,
    pub target_channel_owner_id: NodeId,
    pub target_channel_id: String,
    pub target_thread_id: String,
    pub requested_payload_family: MessagePayloadFamily,
    pub requested_payload_encoding: MessagePayloadEncoding,
    pub requested_payload_compression: MessagePayloadCompression,
    pub requested_payload_bytes: Vec<u8>,
    pub decision_mode: SpeechDecisionMode,
    pub status: SpeechRequestStatus,
    pub resolved_message_id: Option<String>,
    pub created_at: UnixTimestamp,
    pub updated_at: UnixTimestamp,
}

impl SpeechRequestRecord {
    pub fn requested_payload(&self) -> Result<MessagePayload, MessagePayloadCodecError> {
        decode_message_payload(
            self.requested_payload_family,
            self.requested_payload_encoding,
            self.requested_payload_compression,
            &self.requested_payload_bytes,
        )
    }
}


