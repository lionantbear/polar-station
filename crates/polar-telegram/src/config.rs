#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelegramClientProvider {
    BotApi,
    Grammers,
    Mock,
}

impl TelegramClientProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BotApi => "bot_api",
            Self::Grammers => "grammers",
            Self::Mock => "mock",
        }
    }
}

/// Provider configuration without chat routing or workspace identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramProviderConfig {
    pub provider: TelegramClientProvider,
    pub bot_api_base_url: String,
    pub credential_bundle_ref: Option<String>,
    pub session_bundle_ref: Option<String>,
    pub session_dir: Option<String>,
    pub poll_interval_ms: u64,
    pub bots: Vec<TelegramProviderBotConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramProviderBotConfig {
    pub bot_name: String,
    pub credential_bundle_ref: Option<String>,
}
