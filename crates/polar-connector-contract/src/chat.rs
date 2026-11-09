//! Connector operation correlation and durable completion contract.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolarInkChatOperationState {
    Reserved,
    Accepted,
    Completed,
    Failed,
    NotFound,
}

impl PolarInkChatOperationState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Accepted => "accepted",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::NotFound => "not_found",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolarInkChatOperationView {
    pub connector_id: String,
    pub device_id: String,
    pub client_message_id: String,
    pub state: PolarInkChatOperationState,
    pub command_id: Option<String>,
    pub request_message_id: Option<String>,
    pub reply_message_id: Option<String>,
    pub failure_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolarInkChatReserveDisposition {
    Created,
    Existing,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolarInkChatReserveInput {
    pub connector_id: String,
    pub device_id: String,
    pub owner_id: String,
    pub thread_id: String,
    pub client_message_id: String,
    pub text_sha256: String,
    pub now_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolarInkChatReserveOutput {
    pub disposition: PolarInkChatReserveDisposition,
    pub operation: PolarInkChatOperationView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolarInkChatAcceptInput {
    pub connector_id: String,
    pub device_id: String,
    pub client_message_id: String,
    pub command_id: String,
    pub request_message_id: String,
    pub now_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolarInkChatCompleteInput {
    pub request_message_id: String,
    pub reply_message_id: String,
    pub now_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolarInkChatFailInput {
    pub connector_id: String,
    pub device_id: String,
    pub client_message_id: String,
    pub failure_code: String,
    pub now_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolarInkChatResumeInput {
    pub connector_id: String,
    pub device_id: String,
    pub client_message_id: String,
    pub now_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolarInkChatResumeOutput {
    pub operation: PolarInkChatOperationView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_source_markdown: Option<String>,
}
