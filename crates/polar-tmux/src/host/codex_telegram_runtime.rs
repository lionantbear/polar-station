//! Execution-Host channel workers; SQLite remains the only delivery ledger.
use super::application_state::{wait_for_work, wait_configuration, DeliveryChannel};
use super::codex_control::CodexControlService;
use super::imessage_sender::now_ms;
use crate::host_identity::CredentialStore;
use polar_station_settings::CodexControlSettingsV1;
use polar_station_store_sqlite::CodexSubmissionRow;
use polar_telegram::{
    TelegramApiFailure, TelegramBotApiClient, TelegramClient, TelegramClientProvider,
    TelegramOutboundMessage, TelegramProviderBotConfig, TelegramProviderConfig,
};
use polar_tmux_protocol::{ApplicationStateApiV1, CodexChannelStateV1, CodexChannelStatusV1};
use polar_vault::{CredentialBundle, CredentialBundleResolver};
use rand_core::{OsRng, RngCore};
use std::{path::PathBuf, sync::Arc, time::Duration};

const BOT_NAME: &str = "station";

/// Owned by the Host runtime, independent of desktop connections.
pub(crate) struct TelegramRuntime(tokio::task::JoinHandle<()>);
impl Drop for TelegramRuntime {
    fn drop(&mut self) {
        self.0.abort();
    }
}
impl TelegramRuntime {
    pub(crate) async fn shutdown(mut self) {
        self.0.abort();
        let _ = (&mut self.0).await;
    }

    pub(crate) fn start(
        service: Arc<CodexControlService>,
        credentials: Arc<dyn CredentialStore + Send + Sync>,
    ) -> Self {
        Self::spawn(
            service,
            credentials,
            "https://api.telegram.org".into(),
            None,
        )
    }
    pub(super) fn spawn(
        service: Arc<CodexControlService>,
        credentials: Arc<dyn CredentialStore + Send + Sync>,
        origin: String,
        lock_root: Option<PathBuf>,
    ) -> Self {
        Self(tokio::spawn(async move {
            let mut backoff = 1;
            loop {
                let config = service.config.read().await.clone();
                if !config.telegram.enabled {
                    service
                        .channel_status(CodexChannelStateV1::Disabled, None)
                        .await;
                    wait_configuration_change(&service, &config).await;
                    continue;
                }
                service
                    .channel_status(CodexChannelStateV1::Connecting, None)
                    .await;
                // Dropping this future cancels long polls, sends and current RPC waits.
                let result = tokio::select! {
                    result = run_epoch(&service, &config, credentials.clone(), &origin, lock_root.as_ref()) => Some(result),
                    _ = wait_configuration_change(&service, &config) => None,
                };
                let error = result.and_then(Result::err);
                let Some(error) = error else {
                    backoff = 1;
                    continue;
                };
                service
                    .channel_status(CodexChannelStateV1::Error, Some(error.code))
                    .await;
                if let Some(delay) = error.retry_after {
                    let delay = if error.code == "telegram_transport" {
                        let jitter = u64::from(OsRng.next_u32() % 251);
                        Duration::from_millis(backoff * 1000 + jitter)
                    } else {
                        delay
                    };
                    backoff = (backoff * 2).min(30);
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {},
                        _ = wait_configuration_change(&service, &config) => { backoff = 1; },
                    }
                } else {
                    // A webhook, consumer conflict, identity mismatch or missing
                    // credential needs an explicit configuration change.
                    wait_configuration_change(&service, &config).await;
                    backoff = 1;
                }
            }
        }))
    }
}

struct BotLease {
    _lock: crate::local_ipc::UserFileLock,
    store: polar_station_store_sqlite::SqliteCodexStore,
    bot: i64,
    connectors: Vec<String>,
    publisher: super::application_state::ApplicationStatePublisher,
}
impl Drop for BotLease {
    fn drop(&mut self) {
        for id in &self.connectors {
            if self.store.interrupt_connector_io(id, now_ms()).is_err() {
                tracing::warn!("Connector interruption persistence failed");
            }
        }
        self.publisher
            .publish(ApplicationStateApiV1::NotificationV1);
        // Only the owner holding the per-bot lock may recover interrupted I/O.
        if self
            .store
            .interrupt_telegram_io(self.bot, now_ms())
            .is_err()
        {
            tracing::warn!("Telegram interruption receipts could not be persisted");
        }
    }
}

pub(super) struct StationBotResolver(pub(super) Arc<dyn CredentialStore + Send + Sync>);
#[async_trait::async_trait]
impl CredentialBundleResolver for StationBotResolver {
    async fn resolve_credential_bundle(
        &self,
        credential_ref: &str,
    ) -> anyhow::Result<CredentialBundle> {
        let (store, account) = (self.0.clone(), credential_ref.to_owned());
        let token = tokio::task::spawn_blocking(move || {
            store.load("com.polarealm.PolarStation.channels", &account)
        })
        .await
        .map_err(|_| anyhow::anyhow!("credential_unavailable"))?
        .map_err(|_| anyhow::anyhow!("credential_unavailable"))?
        .ok_or_else(|| anyhow::anyhow!("credential_missing"))?;
        Ok(CredentialBundle::TelegramBot { bot_token: token })
    }
}

#[derive(Debug)]
pub(super) struct ChannelFailure {
    pub code: &'static str,
    retry_after: Option<Duration>,
}
impl From<&'static str> for ChannelFailure {
    fn from(code: &'static str) -> Self {
        Self {
            code,
            retry_after: None,
        }
    }
}
fn provider_failure(error: &anyhow::Error) -> ChannelFailure {
    match error.downcast_ref::<TelegramApiFailure>() {
        Some(TelegramApiFailure::ConsumerConflict) => "telegram_consumer_conflict".into(),
        Some(TelegramApiFailure::Unauthorized) => "telegram_unauthorized".into(),
        Some(TelegramApiFailure::RateLimited { retry_after_secs }) => ChannelFailure {
            code: "telegram_rate_limited",
            retry_after: (*retry_after_secs <= 86400)
                .then(|| Duration::from_secs(*retry_after_secs)),
        },
        Some(TelegramApiFailure::Rejected { .. }) => "telegram_rejected".into(),
        None => ChannelFailure {
            code: "telegram_transport",
            retry_after: Some(Duration::from_secs(1)),
        },
    }
}

async fn wait_configuration_change(
    service: &CodexControlService,
    expected: &CodexControlSettingsV1,
) {
    let mut events = service.publisher.subscribe_work();
    let revision = service
        .settings
        .load_message_connectors()
        .ok()
        .map(|s| s.revision);
    loop {
        if *service.config.read().await != *expected { return; }
        match wait_configuration(&mut events).await {
            Ok((kind, changed))
                if kind == super::application_state::ConfigurationKind::Connectors && Some(changed) != revision => { return; }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_))
                if service.settings.load_message_connectors().ok().map(|s| s.revision) != revision => { return; }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            _ => {}
        }
    }
}

async fn run_epoch(
    service: &CodexControlService,
    config: &CodexControlSettingsV1,
    credentials: Arc<dyn CredentialStore + Send + Sync>,
    origin: &str,
    lock_root: Option<&PathBuf>,
) -> Result<(), ChannelFailure> {
    let client = TelegramBotApiClient::from_config(
        &TelegramProviderConfig {
            provider: TelegramClientProvider::BotApi,
            bot_api_base_url: origin.into(),
            credential_bundle_ref: None,
            session_bundle_ref: None,
            session_dir: None,
            poll_interval_ms: 0,
            bots: vec![TelegramProviderBotConfig {
                bot_name: BOT_NAME.into(),
                credential_bundle_ref: config.telegram.bot_credential_ref.clone(),
            }],
        },
        &StationBotResolver(credentials),
    )
    .await
    .map_err(|e| {
        if matches!(
            e.to_string().as_str(),
            "credential_missing" | "credential_unavailable"
        ) {
            ChannelFailure::from("telegram_credential_missing")
        } else {
            provider_failure(&e)
        }
    })?;
    let bot = client
        .runtime_bots()
        .first()
        .and_then(|b| b.telegram_user_id)
        .filter(|id| *id > 0)
        .ok_or("telegram_identity_mismatch")?;
    if config
        .telegram
        .allowed_sources
        .iter()
        .any(|s| s.bot_id != bot)
        || config.telegram.destinations.iter().any(|d| d.bot_id != bot)
    {
        return Err("telegram_identity_mismatch".into());
    }
    let path = match lock_root {
        Some(root) => root.join(format!("telegram-{bot}")),
        None => {
            crate::local_ipc::telegram_lock_socket(bot).map_err(|_| "telegram_lock_unavailable")?
        }
    };
    let consumer = crate::local_ipc::acquire_host_instance_lock(&path)
        .map_err(|_| "telegram_consumer_conflict")?;
    let _lease = BotLease {
        _lock: consumer,
        store: service.ledger.clone(),
        bot,
        connectors: service
            .settings
            .load_message_connectors()
            .map_err(|_| "settings_unavailable")?
            .connectors
            .into_iter()
            .filter(|c| {
                c.host_id == service.owner
                    && c.connection_ref.is_none()
                    && c.provider == polar_station_settings::ConnectorProviderKind::Telegram
            })
            .map(|c| c.id)
            .collect(),
        publisher: service.publisher.clone(),
    };
    // A settings change can cancel I/O for a connector created after the old
    // lease was acquired. Recover the current set while holding the bot lock.
    for id in &_lease.connectors {
        service
            .ledger
            .interrupt_connector_io(id, now_ms())
            .map_err(|_| "ledger_unavailable")?;
    }
    if client
        .has_webhook(BOT_NAME)
        .await
        .map_err(|e| provider_failure(&e))?
    {
        return Err("telegram_webhook_active".into());
    }
    service
        .channel_status(CodexChannelStateV1::Ready, None)
        .await;
    tokio::select! {
        result = receive_loop(service,&client,bot) => result,
        result = send_loop(service,&client,bot) => result,
    }
}
async fn receive_loop(
    service: &CodexControlService,
    client: &TelegramBotApiClient,
    bot: i64,
) -> Result<(), ChannelFailure> {
    loop {
        // Resume only received rows; submitting/unknown never trigger another write.
        service.resume_received_telegram(bot).await?;
        let offset = service
            .ledger
            .telegram_cursor(bot)
            .map_err(|_| "ledger_unavailable")?
            .map(u64::try_from)
            .transpose()
            .map_err(|_| "invalid_telegram_cursor")?;
        let batch = client
            .poll_batch(BOT_NAME, offset, 30)
            .await
            .map_err(|e| provider_failure(&e))?;
        service.receive_telegram(bot, batch).await?;
    }
}
async fn send_loop(
    service: &CodexControlService,
    client: &TelegramBotApiClient,
    bot: i64,
) -> Result<(), ChannelFailure> {
    let mut events = service.publisher.subscribe_work();
    let mut retention = tokio::time::interval(Duration::from_secs(60));
    retention.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        service.resume_received_telegram(bot).await?;
        service
            .connector_service()
            .deliver_telegram(client, bot, BOT_NAME)
            .await?;
        if service.config.read().await.enabled {
            service.deliver_telegram(client, bot, BOT_NAME).await?;
            service.acknowledge_telegram(client, bot, BOT_NAME).await?;
        }
        // Wake on durable work, including lag recovery, with a bounded retention timer.
        tokio::select! {
            result = wait_for_work(&mut events, DeliveryChannel::Telegram) => {
                if matches!(result, Err(tokio::sync::broadcast::error::RecvError::Closed)) { return Ok(()); }
            },
            _ = retention.tick() => {
                service.ledger.expire_transient_text(now_ms()).map_err(|_| "ledger_unavailable")?;
            },
        }
    }
}

impl CodexControlService {
    async fn channel_status(&self, state: CodexChannelStateV1, code: Option<&str>) {
        let next = CodexChannelStatusV1 {
            state,
            error_code: code.map(str::to_owned),
        };
        let mut status = self.telegram_status.write().await;
        if *status != next {
            *status = next;
            self.publisher.publish(ApplicationStateApiV1::CodexV1);
        }
    }
    pub(super) async fn deliver_telegram(
        &self,
        client: &TelegramBotApiClient,
        bot: i64,
        name: &str,
    ) -> Result<(), ChannelFailure> {
        for row in self
            .ledger
            .pending_telegram_notifications(bot, now_ms())
            .map_err(|_| "ledger_unavailable")?
        {
            let body = {
                let _gate = self.operations.lock().await;
                let config = self.config.read().await;
                if !config.enabled || !config.telegram.enabled {
                    return Err("telegram_disabled".into());
                }
                let destination_current = config.telegram.destinations.iter().any(|d| {
                    Some(d.bot_id) == row.bot_id
                        && Some(d.chat_id) == row.chat_id
                        && serde_json::to_string(d).ok().as_ref() == Some(&row.destination_key)
                });
                let binding_current = match self.binding_row(&row.binding_id) {
                    Ok(binding) => binding.revision == row.binding_revision,
                    Err("binding_not_found") => false,
                    Err(code) => return Err(code.into()),
                };
                if !destination_current || !binding_current {
                    self.ledger
                        .transition_notification(
                            &row.id,
                            "pending",
                            "failed",
                            None,
                            Some("binding_conflict".into()),
                        )
                        .map_err(|_| "ledger_unavailable")?;
                    continue;
                }
                let text = row
                    .bounded_body
                    .clone()
                    .ok_or("notification_body_missing")?;
                if !self
                    .ledger
                    .transition_notification(&row.id, "pending", "sending", None, None)
                    .map_err(|_| "ledger_unavailable")?
                {
                    continue;
                }
                text
            };
            let result = client
                .send_text(TelegramOutboundMessage {
                    bot_name: name.into(),
                    chat_id: row.chat_id.ok_or("invalid_destination")?,
                    text: body,
                })
                .await;
            let (status, message_id, error) = send_result(&result);
            self.ledger
                .transition_notification(
                    &row.id,
                    "sending",
                    if status == "rejected" {
                        "failed"
                    } else {
                        status
                    },
                    message_id,
                    error,
                )
                .map_err(|_| "ledger_unavailable")?;
            self.publisher.publish(ApplicationStateApiV1::CodexV1);
            if let Err(error) = result {
                if matches!(
                    error.downcast_ref::<TelegramApiFailure>(),
                    Some(
                        TelegramApiFailure::RateLimited { .. }
                            | TelegramApiFailure::Unauthorized
                            | TelegramApiFailure::ConsumerConflict
                    )
                ) {
                    return Err(provider_failure(&error));
                }
            }
        }
        Ok(())
    }

    pub(super) async fn acknowledge_telegram(
        &self,
        client: &TelegramBotApiClient,
        bot: i64,
        name: &str,
    ) -> Result<(), ChannelFailure> {
        for row in self
            .ledger
            .pending_telegram_acknowledgements(bot, now_ms())
            .map_err(|_| "ledger_unavailable")?
        {
            let _gate = self.operations.lock().await;
            let config = self.config.read().await;
            if !config.enabled || !config.telegram.enabled {
                return Err("telegram_disabled".into());
            }
            if !self.telegram_row_authorized(&config.telegram, &row)
                || row
                    .message_id
                    .is_none_or(|id| id <= 0 || id > i64::from(i32::MAX))
            {
                self.ledger
                    .transition_telegram_ack(&row.intent_id, "pending", "suppressed", None, None)
                    .map_err(|_| "ledger_unavailable")?;
                continue;
            }
            if !self
                .ledger
                .transition_telegram_ack(&row.intent_id, "pending", "sending", None, None)
                .map_err(|_| "ledger_unavailable")?
            {
                continue;
            }
            drop(config);
            drop(_gate);
            let result = client
                .send_text_reply(
                    TelegramOutboundMessage {
                        bot_name: name.into(),
                        chat_id: row.chat_id.ok_or("source_denied")?,
                        text: acknowledgement(&row).into(),
                    },
                    row.message_id.and_then(|id| i32::try_from(id).ok()),
                )
                .await;
            let (status, message_id, error) = send_result(&result);
            self.ledger
                .transition_telegram_ack(&row.intent_id, "sending", status, message_id, error)
                .map_err(|_| "ledger_unavailable")?;
            self.publisher.publish(ApplicationStateApiV1::CodexV1);
            if let Err(error) = result {
                if matches!(
                    error.downcast_ref::<TelegramApiFailure>(),
                    Some(
                        TelegramApiFailure::RateLimited { .. }
                            | TelegramApiFailure::Unauthorized
                            | TelegramApiFailure::ConsumerConflict
                    )
                ) {
                    return Err(provider_failure(&error));
                }
            }
        }
        Ok(())
    }
}

fn send_result(
    result: &anyhow::Result<polar_telegram::TelegramSentMessage>,
) -> (&'static str, Option<i64>, Option<String>) {
    match result {
        Ok(sent) => ("sent", Some(i64::from(sent.message_id)), None),
        Err(error) => {
            let status = match error.downcast_ref::<TelegramApiFailure>() {
                Some(TelegramApiFailure::RateLimited { .. }) => "pending",
                Some(TelegramApiFailure::Rejected { code }) if *code >= 500 => "unknown",
                Some(_) => "rejected",
                None => "unknown",
            };
            (status, None, Some(provider_failure(error).code.into()))
        }
    }
}
fn acknowledgement(row: &CodexSubmissionRow) -> &'static str {
    match row.status.as_str() {
        "accepted" => "已接受并排队，后续结果将由原会话通知。",
        "unknown" => "结果未知：请在 Station 查看回执。请勿重复发送。",
        _ => match row.error_code.as_deref() {
            Some("notification_unknown") => "未投递：这条消息不是 Station 可回复的完成通知。请回复带会话名字和 ID 的「Polar Station · 已完成」消息；旧 hook 通知和投递回执不能接收回复。",
            Some("notification_expired" | "expired") => "未投递：通知或回复已过期。",
            Some("unsupported_message") => "未投递：只支持普通聊天中的新文字回复。",
            _ => "未投递：原会话已失效或当前不可用，请在 Station 检查。",
        },
    }
}
