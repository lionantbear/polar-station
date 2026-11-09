use polar_identity::UnixTimestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramWorkspaceRecord {
    pub eve_bot_name: String,
    pub chat_id: Option<i64>,
    pub current_bot_name: Option<String>,
    pub current_thread_id: Option<String>,
    pub updated_at: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramTaskBotBindingRecord {
    pub bot_name: String,
    pub thread_id: String,
    pub updated_at: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramSessionRecord {
    pub session_id: String,
    pub bot_name: String,
    pub chat_id: i64,
    pub remote_user_id: Option<i64>,
    pub remote_display_name: Option<String>,
    pub remote_username: Option<String>,
    pub current_chat_side_name: Option<String>,
    pub current_thread_id: Option<String>,
    pub updated_at: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramPendingChatBridgeRequestRecord {
    pub request_id: String,
    pub session_id: String,
    pub bot_name: String,
    pub chat_id: i64,
    pub remote_user_id: Option<i64>,
    pub remote_display_name: Option<String>,
    pub remote_username: Option<String>,
    pub requested_text: String,
    pub attempt_count: u64,
    pub first_received_at: UnixTimestamp,
    pub last_received_at: UnixTimestamp,
    pub suppressed_duplicate_count: u64,
    pub status: String,
    pub target_thread_id: Option<String>,
    pub received_at: UnixTimestamp,
    pub resolved_at: Option<UnixTimestamp>,
    pub updated_at: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatPendingRequestSnapshot {
    pub request_id: String,
    pub session_id: String,
    pub endpoint_name: String,
    pub chat_id: i64,
    pub remote_user_id: String,
    pub remote_display_name: Option<String>,
    pub remote_username: Option<String>,
    pub requested_text: String,
    pub attempt_count: u64,
    pub first_received_at: u64,
    pub last_received_at: u64,
    pub suppressed_duplicate_count: u64,
    pub status: String,
    pub target_thread_id: Option<String>,
    pub received_at: u64,
    pub resolved_at: Option<u64>,
}


