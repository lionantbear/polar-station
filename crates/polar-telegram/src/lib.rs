#[cfg(feature = "bot-api")]
mod bot_api;
mod client;
mod config;
#[cfg(feature = "grammers")]
mod grammers;
#[cfg(feature = "mock")]
mod mock;
mod runtime;

#[cfg(feature = "bot-api")]
pub use bot_api::{TelegramApiFailure, TelegramBotApiBot, TelegramBotApiClient};
pub use client::{
    build_client_for_instance, build_client_from_env, BoxedTelegramClient, TelegramBotRuntimeView,
    TelegramClient,
};
pub use config::{TelegramClientProvider, TelegramProviderBotConfig, TelegramProviderConfig};
#[cfg(feature = "grammers")]
pub use grammers::GrammersTelegramClient;
#[cfg(feature = "mock")]
pub use mock::MockTelegramClient;
pub use runtime::{
    TelegramAttachmentPayload, TelegramDownloadedFile, TelegramInboundAttachment,
    TelegramInboundEnvelope, TelegramInboundMessage, TelegramMessageMetadata,
    TelegramOutboundDocument, TelegramOutboundMessage, TelegramOutboundPhoto,
    TelegramProviderUpdate, TelegramSentMessage, TelegramTypingAction, TelegramUpdateBatch,
};
