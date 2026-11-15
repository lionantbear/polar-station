pub mod chat;
pub mod conversation;
pub use conversation::*;
pub mod inquiry;
pub use inquiry::ConnectorInquirySessionView;

pub mod attachments;

pub use attachments::*;

use serde::{Deserialize, Serialize};

pub const MESSAGE_INGRESS_CAPABILITY: &str = "message_ingress";
pub const MESSAGE_EGRESS_CAPABILITY: &str = "message_egress";
pub const NOTIFY_EGRESS_CAPABILITY: &str = "notify_egress";
pub const DEEP_LINK_CAPABILITY: &str = "deep_link";
pub const ATTACHMENT_IMPORT_CAPABILITY: &str = "attachment_import";
pub const ATTACHMENT_EXPORT_CAPABILITY: &str = "attachment_export";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectorCapability {
    MessageIngress,
    MessageEgress,
    NotifyEgress,
    DeepLink,
    AttachmentImport,
    AttachmentExport,
}

impl ConnectorCapability {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MessageIngress => MESSAGE_INGRESS_CAPABILITY,
            Self::MessageEgress => MESSAGE_EGRESS_CAPABILITY,
            Self::NotifyEgress => NOTIFY_EGRESS_CAPABILITY,
            Self::DeepLink => DEEP_LINK_CAPABILITY,
            Self::AttachmentImport => ATTACHMENT_IMPORT_CAPABILITY,
            Self::AttachmentExport => ATTACHMENT_EXPORT_CAPABILITY,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectorProviderKind {
    WebPublicHttp,
    PolarInk,
    Telegram,
    Discord,
    Email,
    Imessage,
    NativePush,
    Mock,
}

impl ConnectorProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WebPublicHttp => "web_public_http",
            Self::PolarInk => "polar_ink",
            Self::Telegram => "telegram",
            Self::Discord => "discord",
            Self::Email => "email",
            Self::Imessage => "imessage",
            Self::NativePush => "native_push",
            Self::Mock => "mock",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "web_public_http" => Some(Self::WebPublicHttp),
            "polar_ink" => Some(Self::PolarInk),
            "telegram" => Some(Self::Telegram),
            "discord" => Some(Self::Discord),
            "email" => Some(Self::Email),
            "imessage" => Some(Self::Imessage),
            "native_push" => Some(Self::NativePush),
            "mock" => Some(Self::Mock),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectorDriverKind {
    WebPublicHttp,
    PolarInkWss,
    TelegramBotApi,
    TelegramGrammers,
    DiscordBot,
    Smtp,
    Imap,
    NativePush,
    Mock,
}

impl ConnectorDriverKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WebPublicHttp => "web_public_http",
            Self::PolarInkWss => "polar_ink_wss",
            Self::TelegramBotApi => "telegram_bot_api",
            Self::TelegramGrammers => "telegram_grammers",
            Self::DiscordBot => "discord_bot",
            Self::Smtp => "smtp",
            Self::Imap => "imap",
            Self::NativePush => "native_push",
            Self::Mock => "mock",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "web_public_http" => Some(Self::WebPublicHttp),
            "polar_ink_wss" => Some(Self::PolarInkWss),
            "telegram_bot_api" => Some(Self::TelegramBotApi),
            "telegram_grammers" => Some(Self::TelegramGrammers),
            "discord_bot" => Some(Self::DiscordBot),
            "smtp" => Some(Self::Smtp),
            "imap" => Some(Self::Imap),
            "native_push" => Some(Self::NativePush),
            "mock" => Some(Self::Mock),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorInstanceConfig {
    pub connector_id: String,
    pub provider_kind: ConnectorProviderKind,
    pub bot_public_id: Option<String>,
    pub display_name: Option<String>,
    pub driver_kind: ConnectorDriverKind,
    pub server_address: Option<String>,
    pub enabled: bool,
    pub credential_bundle_ref: Option<String>,
    pub session_bundle_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorBinding {
    pub connector_id: String,
    pub capability: ConnectorCapability,
    pub target: String,
}
