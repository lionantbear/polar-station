//! Telegram-native transport models; chat routing belongs to polar-chat-bridge.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelegramAttachmentPayload {
    Bytes(Vec<u8>),
    FilePath(std::path::PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelegramInboundAttachment {
    Photo {
        file_id: String,
        file_unique_id: Option<String>,
    },
    Document {
        file_id: String,
        file_unique_id: Option<String>,
        file_name: Option<String>,
        mime_type: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramTypingAction {
    pub bot_name: String,
    pub chat_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramOutboundPhoto {
    pub bot_name: String,
    pub chat_id: i64,
    pub payload: TelegramAttachmentPayload,
    pub caption: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramOutboundDocument {
    pub bot_name: String,
    pub chat_id: i64,
    pub payload: TelegramAttachmentPayload,
    pub filename: Option<String>,
    pub caption: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramDownloadedFile {
    pub filename: Option<String>,
    pub mime_type: Option<String>,
    pub bytes: Vec<u8>,
}

/// Telegram-native identifiers and update provenance, without routing policy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TelegramMessageMetadata {
    pub message_id: Option<i32>,
    pub reply_to_message_id: Option<i32>,
    pub message_thread_id: Option<i64>,
    pub reply_to_message_thread_id: Option<i64>,
    pub chat_type: Option<String>,
    pub forwarded: bool,
    pub edited: bool,
}

/// A batch is acknowledged only when the caller supplies next_offset on a later poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramUpdateBatch {
    pub bot_name: String,
    pub next_offset: Option<u64>,
    pub updates: Vec<TelegramProviderUpdate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramProviderUpdate {
    pub update_id: u64,
    /// Unsupported updates retain their ID so durable rejection can advance the cursor.
    pub message: Option<TelegramInboundMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramInboundMessage {
    pub metadata: TelegramMessageMetadata,
    pub bot_name: String,
    pub chat_id: i64,
    pub remote_user_id: Option<i64>,
    pub remote_display_name: Option<String>,
    pub remote_username: Option<String>,
    pub text: String,
    pub attachments: Vec<TelegramInboundAttachment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramInboundEnvelope {
    pub update_id: u64,
    pub message: TelegramInboundMessage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramOutboundMessage {
    pub bot_name: String,
    pub chat_id: i64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramSentMessage {
    pub bot_name: String,
    pub chat_id: i64,
    pub message_id: i32,
}
