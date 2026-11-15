//! Local configuration workflow; credentials and provider admission stay in Host.
use super::codex_control::{loaded, CodexControlService, ControlResult};
use super::codex_endpoint::{CodexEndpoint, RpcFailure};
use polar_station_settings::{
    CodexControlModeV1, CodexEndpointSettingsV1, TelegramAllowedSourceV1,
    TelegramControlSettingsV1, TelegramDestinationV1,
};
use polar_telegram::{
    TelegramBotApiClient, TelegramClient, TelegramClientProvider, TelegramProviderBotConfig,
    TelegramProviderConfig,
};
use polar_tmux_protocol::CodexResultV1;
use polar_vault::{CredentialBundle, CredentialBundleResolver};
use std::path::PathBuf;
use zeroize::Zeroizing;

const LEGACY_SERVICE: &str = "codex-cli-telegram-hook";
const CHANNEL_SERVICE: &str = "com.polarealm.PolarStation.channels";

struct CurrentBot(Zeroizing<String>);
#[async_trait::async_trait]
impl CredentialBundleResolver for CurrentBot {
    async fn resolve_credential_bundle(&self, _: &str) -> anyhow::Result<CredentialBundle> {
        Ok(CredentialBundle::TelegramBot {
            bot_token: self.0.to_string(),
        })
    }
}

impl CodexControlService {
    pub(super) async fn setup_current_telegram(
        &self,
        revision: u64,
        sender: Option<i64>,
    ) -> ControlResult<CodexResultV1> {
        let home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".codex")))
            .ok_or("codex_home_missing")?;
        let socket = codex_app_server_client::app_server_control_socket_path(&home)
            .map_err(|_| "codex_socket_invalid")?;
        self.setup_current_at(
            revision,
            sender,
            "https://api.telegram.org",
            socket.as_path().to_str().ok_or("codex_socket_invalid")?,
        )
        .await
    }

    // Caller holds operations; test injection changes only provider/socket locations.
    pub(super) async fn setup_current_at(
        &self,
        revision: u64,
        sender: Option<i64>,
        origin: &str,
        socket: &str,
    ) -> ControlResult<CodexResultV1> {
        let mut settings = self.config.read().await.clone();
        if settings.revision != revision {
            return Err("settings_conflict");
        }
        if settings.telegram.bot_credential_ref.is_some() {
            if settings
                .telegram
                .destinations
                .iter()
                .any(|d| d.id == "current")
                && settings.endpoints.iter().any(|e| {
                    e.id == "local-codex"
                        && e.owner_host_id == self.owner
                        && e.socket_path == socket
                })
                && sender.is_none_or(|id| {
                    settings
                        .telegram
                        .allowed_sources
                        .iter()
                        .any(|s| s.sender_id == id)
                })
            {
                return Ok(CodexResultV1::Settings { settings });
            }
            return Err("telegram_already_configured");
        }
        if !settings.telegram.allowed_sources.is_empty()
            || !settings.telegram.destinations.is_empty()
            || settings.endpoints.iter().any(|e| e.id == "local-codex")
        {
            return Err("setup_conflict");
        }
        let store = self.peer_credentials.clone();
        let (token, chat) = tokio::task::spawn_blocking(move || {
            let token = Zeroizing::new(
                store
                    .load(LEGACY_SERVICE, "bot-token")
                    .map_err(|_| "credential_unavailable")?
                    .ok_or("credential_missing")?,
            );
            let chat = Zeroizing::new(
                store
                    .load(LEGACY_SERVICE, "chat-id")
                    .map_err(|_| "credential_unavailable")?
                    .ok_or("credential_missing")?,
            );
            Ok::<_, &'static str>((
                token,
                chat.parse::<i64>().map_err(|_| "telegram_chat_invalid")?,
            ))
        })
        .await
        .map_err(|_| "credential_unavailable")??;
        let client = TelegramBotApiClient::from_config(
            &TelegramProviderConfig {
                provider: TelegramClientProvider::BotApi,
                bot_api_base_url: origin.into(),
                credential_bundle_ref: None,
                session_bundle_ref: None,
                session_dir: None,
                poll_interval_ms: 0,
                bots: vec![TelegramProviderBotConfig {
                    bot_name: "current".into(),
                    credential_bundle_ref: Some("current".into()),
                }],
            },
            &CurrentBot(token.clone()),
        )
        .await
        .map_err(|_| "telegram_preflight_failed")?;
        let bot_id = client
            .runtime_bots()
            .first()
            .and_then(|b| b.telegram_user_id)
            .filter(|id| *id > 0)
            .ok_or("telegram_identity_invalid")?;
        let (verified_chat, kind) = client
            .chat_identity("current", chat)
            .await
            .map_err(|_| "telegram_chat_unavailable")?;
        if verified_chat != chat {
            return Err("telegram_chat_mismatch");
        }
        let sender_id = match kind.as_str() {
            "private" if chat > 0 && sender.is_none_or(|s| s == chat) => chat,
            "group" | "supergroup" => sender
                .filter(|s| *s > 0)
                .ok_or("telegram_allowed_sender_required")?,
            _ => return Err("telegram_chat_not_authorized"),
        };
        if client
            .has_webhook("current")
            .await
            .map_err(|_| "telegram_preflight_failed")?
        {
            return Err("telegram_webhook_present");
        }
        // Observe the existing daemon only; this adapter never bootstraps it.
        let (events, _receiver) = tokio::sync::mpsc::channel(64);
        let endpoint = CodexEndpoint::connect("setup-probe".into(), 1, socket, events, None)
            .await
            .map_err(RpcFailure::code)?;
        loaded(&endpoint).await?;
        drop(endpoint);
        let reference = format!(
            "telegram-current-{}",
            crate::host_identity::random_uuid_v4()
        );
        settings.enabled = true;
        settings.endpoints.push(CodexEndpointSettingsV1 {
            id: "local-codex".into(),
            execution_target_id: "local".into(),
            control_mode: CodexControlModeV1::Local,
            owner_host_id: self.owner.clone(),
            socket_path: socket.into(),
            peer_connection_ref: None,
            remote_endpoint_id: None,
            remote_target_id: None,
        });
        settings.telegram = TelegramControlSettingsV1 {
            enabled: false,
            bot_credential_ref: Some(reference.clone()),
            allowed_sources: vec![TelegramAllowedSourceV1 {
                bot_id,
                chat_id: chat,
                sender_id,
            }],
            destinations: vec![TelegramDestinationV1 {
                id: "current".into(),
                bot_id,
                chat_id: chat,
            }],
        };
        settings.validate().map_err(|_| "invalid_settings")?;
        let store = self.peer_credentials.clone();
        tokio::task::spawn_blocking(move || store.save(CHANNEL_SERVICE, &reference, &token))
            .await
            .map_err(|_| "credential_unavailable")?
            .map_err(|_| "credential_unavailable")?;
        self.save_control_settings(revision, settings).await
    }
}
