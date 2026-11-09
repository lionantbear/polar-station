use anyhow::{anyhow, Result};
use async_trait::async_trait;
use polar_vault::{CredentialBundle, CredentialBundleResolver};
use serde::{Deserialize, Serialize};

#[cfg(feature = "bot-api")]
use crate::bot_api::TelegramBotApiClient;
use crate::config::{TelegramClientProvider, TelegramProviderBotConfig, TelegramProviderConfig};
#[cfg(feature = "grammers")]
use crate::grammers::GrammersTelegramClient;
#[cfg(feature = "mock")]
use crate::mock::MockTelegramClient;
use crate::runtime::{
    TelegramDownloadedFile, TelegramInboundEnvelope, TelegramOutboundDocument,
    TelegramOutboundMessage, TelegramOutboundPhoto, TelegramSentMessage, TelegramTypingAction,
};

// Transport boundary:
// - This trait exposes Telegram transport primitives only.
// - Provider-specific send/recv/edit/delete/typing/media operations belong here.
// - Cross-channel chat bridge semantics do not belong here.
//
// Do not add shared attachment/progress/business truth to this file.
// Those semantics must live in transport-agnostic chat-bridge-facing types so
// Telegram and future transports such as Discord can reuse them.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelegramBotRuntimeView {
    pub bot_name: String,
    pub telegram_user_id: Option<i64>,
    pub telegram_name: Option<String>,
    pub telegram_username: Option<String>,
    pub authorized: bool,
    pub session_path: Option<String>,
    pub session_present: bool,
}

#[async_trait]
pub trait TelegramClient: Send {
    fn is_realtime(&self) -> bool {
        false
    }

    fn runtime_bots(&self) -> Vec<TelegramBotRuntimeView> {
        Vec::new()
    }

    async fn poll_updates(&mut self) -> Result<Vec<TelegramInboundEnvelope>>;

    async fn recv_update(&mut self) -> Result<Option<TelegramInboundEnvelope>> {
        Ok(self.poll_updates().await?.into_iter().next())
    }

    async fn send_message(
        &mut self,
        message: TelegramOutboundMessage,
    ) -> Result<TelegramSentMessage>;

    async fn edit_message(&mut self, message: &TelegramSentMessage, text: String) -> Result<()>;

    async fn delete_message(&mut self, message: &TelegramSentMessage) -> Result<()>;

    async fn send_typing(&mut self, action: TelegramTypingAction) -> Result<()>;

    async fn send_photo(&mut self, photo: TelegramOutboundPhoto) -> Result<TelegramSentMessage>;

    async fn send_document(
        &mut self,
        document: TelegramOutboundDocument,
    ) -> Result<TelegramSentMessage>;

    async fn download_file(
        &mut self,
        bot_name: &str,
        file_id: &str,
    ) -> Result<TelegramDownloadedFile>;
}

#[async_trait]
impl<T> TelegramClient for Box<T>
where
    T: TelegramClient + ?Sized + Send,
{
    fn is_realtime(&self) -> bool {
        (**self).is_realtime()
    }

    fn runtime_bots(&self) -> Vec<TelegramBotRuntimeView> {
        (**self).runtime_bots()
    }

    async fn poll_updates(&mut self) -> Result<Vec<TelegramInboundEnvelope>> {
        (**self).poll_updates().await
    }

    async fn recv_update(&mut self) -> Result<Option<TelegramInboundEnvelope>> {
        (**self).recv_update().await
    }

    async fn send_message(
        &mut self,
        message: TelegramOutboundMessage,
    ) -> Result<TelegramSentMessage> {
        (**self).send_message(message).await
    }

    async fn edit_message(&mut self, message: &TelegramSentMessage, text: String) -> Result<()> {
        (**self).edit_message(message, text).await
    }

    async fn delete_message(&mut self, message: &TelegramSentMessage) -> Result<()> {
        (**self).delete_message(message).await
    }

    async fn send_typing(&mut self, action: TelegramTypingAction) -> Result<()> {
        (**self).send_typing(action).await
    }

    async fn send_photo(&mut self, photo: TelegramOutboundPhoto) -> Result<TelegramSentMessage> {
        (**self).send_photo(photo).await
    }

    async fn send_document(
        &mut self,
        document: TelegramOutboundDocument,
    ) -> Result<TelegramSentMessage> {
        (**self).send_document(document).await
    }

    async fn download_file(
        &mut self,
        bot_name: &str,
        file_id: &str,
    ) -> Result<TelegramDownloadedFile> {
        (**self).download_file(bot_name, file_id).await
    }
}

pub type BoxedTelegramClient = Box<dyn TelegramClient>;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ResolvedTelegramBotConfig {
    pub bot_name: String,
    pub bot_token: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ResolvedTelegramRuntimeConfig {
    pub upload_directory: Option<std::path::PathBuf>,
    pub provider: TelegramClientProvider,
    pub bot_api_base_url: String,
    pub api_id: Option<String>,
    pub api_hash: Option<String>,
    pub session_bundle_ref: Option<String>,
    pub session_dir: Option<String>,
    pub poll_interval_ms: u64,
    pub bots: Vec<ResolvedTelegramBotConfig>,
}

pub async fn build_client_from_env(
    config: &TelegramProviderConfig,
    resolver: &dyn CredentialBundleResolver,
) -> Result<BoxedTelegramClient> {
    let resolved = resolve_runtime_config(config, resolver).await?;
    build_resolved_client(resolved).await
}

/// Managed hosts bind all provider-owned files before starting a client.
pub async fn build_client_for_instance(
    config: &TelegramProviderConfig,
    resolver: &dyn CredentialBundleResolver,
    instance: &polar_runtime_paths::InstancePathPlan,
) -> Result<BoxedTelegramClient> {
    use polar_runtime_paths::{RuntimeArea, RuntimeModule};
    let mut resolved = resolve_runtime_config(config, resolver).await?;
    if matches!(
        resolved.provider,
        TelegramClientProvider::Grammers | TelegramClientProvider::Mock
    ) {
        let expected = instance.directory_path(RuntimeModule::Telegram, RuntimeArea::Secrets);
        if let Some(actual) = &resolved.session_dir {
            anyhow::ensure!(
                std::path::Path::new(actual) == expected,
                "telegram session_dir differs from its instance binding"
            );
        }
        let sessions = instance.open_directory(RuntimeModule::Telegram, RuntimeArea::Secrets)?;
        let uploads = instance.open_directory(RuntimeModule::Telegram, RuntimeArea::Tmp)?;
        resolved.session_dir = Some(sessions.path().display().to_string());
        resolved.upload_directory = Some(uploads.path().to_path_buf());
    }
    build_resolved_client(resolved).await
}

async fn build_resolved_client(
    resolved: ResolvedTelegramRuntimeConfig,
) -> Result<BoxedTelegramClient> {
    match resolved.provider {
        // Current environment note:
        // - `telegramtest` is configured to use `Grammers`
        // - `BotApi` stays available as a supported provider surface, but it
        //   is not the main validation path for Telegram changes
        #[cfg(feature = "bot-api")]
        TelegramClientProvider::BotApi => {
            Ok(Box::new(TelegramBotApiClient::from_env(&resolved).await?))
        }
        #[cfg(not(feature = "bot-api"))]
        TelegramClientProvider::BotApi => Err(anyhow!("bot-api provider is not enabled")),
        #[cfg(feature = "grammers")]
        TelegramClientProvider::Grammers => {
            Ok(Box::new(GrammersTelegramClient::from_env(&resolved).await?))
        }
        #[cfg(not(feature = "grammers"))]
        TelegramClientProvider::Grammers => Err(anyhow!("grammers provider is not enabled")),
        #[cfg(feature = "mock")]
        TelegramClientProvider::Mock => {
            Ok(Box::new(MockTelegramClient::from_env(&resolved).await?))
        }
        #[cfg(not(feature = "mock"))]
        TelegramClientProvider::Mock => Err(anyhow!("mock provider is not enabled")),
    }
}

pub(crate) async fn resolve_runtime_config(
    config: &TelegramProviderConfig,
    resolver: &dyn CredentialBundleResolver,
) -> Result<ResolvedTelegramRuntimeConfig> {
    let (api_id, api_hash) = resolve_grammers_bundle(config, resolver).await?;
    let mut bots = Vec::with_capacity(config.bots.len());
    for bot in &config.bots {
        bots.push(ResolvedTelegramBotConfig {
            bot_name: bot.bot_name.clone(),
            bot_token: resolve_bot_bundle(bot, resolver).await?,
        });
    }
    Ok(ResolvedTelegramRuntimeConfig {
        upload_directory: None,
        provider: config.provider,
        bot_api_base_url: config.bot_api_base_url.clone(),
        api_id,
        api_hash,
        session_bundle_ref: config.session_bundle_ref.clone(),
        session_dir: config.session_dir.clone(),
        poll_interval_ms: config.poll_interval_ms,
        bots,
    })
}

async fn resolve_grammers_bundle(
    config: &TelegramProviderConfig,
    resolver: &dyn CredentialBundleResolver,
) -> Result<(Option<String>, Option<String>)> {
    let Some(credential_bundle_ref) = config.credential_bundle_ref.as_deref() else {
        return Ok((None, None));
    };
    match resolver
        .resolve_credential_bundle(credential_bundle_ref)
        .await?
    {
        CredentialBundle::TelegramGrammers { api_id, api_hash } => {
            Ok((Some(api_id), Some(api_hash)))
        }
        _ => Err(anyhow!(
            "telegram runtime credential bundle type is unsupported"
        )),
    }
}

async fn resolve_bot_bundle(
    bot: &TelegramProviderBotConfig,
    resolver: &dyn CredentialBundleResolver,
) -> Result<Option<String>> {
    let Some(credential_bundle_ref) = bot.credential_bundle_ref.as_deref() else {
        return Ok(None);
    };
    match resolver
        .resolve_credential_bundle(credential_bundle_ref)
        .await?
    {
        CredentialBundle::TelegramBot { bot_token } => Ok(Some(bot_token)),
        _ => Err(anyhow!(
            "telegram bot credential bundle type is unsupported"
        )),
    }
}
