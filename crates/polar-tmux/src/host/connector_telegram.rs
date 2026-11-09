//! Native private-topic provider adapter; shares the existing single bot consumer.
use super::{imessage_sender::now_ms, message_connector::ConnectorService};
use polar_station_settings::ConnectorProviderKind;
use polar_station_store_sqlite::ConnectorInboundRow;
use polar_telegram::{
    TelegramApiFailure, TelegramBotApiClient, TelegramInboundMessage, TelegramOutboundMessage,
};
use polar_tmux_protocol::{stable_connector_id, validate_codex_input, ApplicationStateApiV1};
use sha2::{Digest, Sha256};
impl ConnectorService {
    pub(super) fn telegram_inbound(
        &self,
        bot: i64,
        message: &TelegramInboundMessage,
    ) -> Result<Option<ConnectorInboundRow>, &'static str> {
        let m = &message.metadata;
        let Some(topic) = m.message_thread_id else {
            return Ok(None);
        };
        if topic <= 0
            || m.edited
            || m.forwarded
            || !message.attachments.is_empty()
            || m.chat_type.as_deref() != Some("private")
            || validate_codex_input(&message.text).is_err()
        {
            tracing::warn!(
                positive_topic = topic > 0,
                edited = m.edited,
                forwarded = m.forwarded,
                attachments = !message.attachments.is_empty(),
                private_chat = m.chat_type.as_deref() == Some("private"),
                valid_text = validate_codex_input(&message.text).is_ok(),
                "Telegram topic input rejected: unsupported message shape"
            );
            return Ok(None);
        }
        let Some(mid) = m.message_id.filter(|id| *id > 0) else {
            return Ok(None);
        };
        if m.reply_to_message_thread_id.is_some_and(|id| id != topic) {
            return Ok(None);
        }
        let config = self
            .settings
            .load_codex_control()
            .map_err(|_| "settings_unavailable")?;
        if !config.telegram.enabled
            || !config.telegram.allowed_sources.iter().any(|s| {
                s.bot_id == bot
                    && s.chat_id == message.chat_id
                    && Some(s.sender_id) == message.remote_user_id
            })
        {
            tracing::warn!(
                sender_present = message.remote_user_id.is_some(),
                "Telegram topic input rejected: source denied"
            );
            return Ok(None);
        }
        let settings = self
            .settings
            .load_message_connectors()
            .map_err(|_| "settings_unavailable")?;
        for c in settings.connectors.iter().filter(|c| {
            c.enabled
                && c.host_id == self.host_id
                && c.connection_ref.is_none()
                && c.provider == ConnectorProviderKind::Telegram
        }) {
            for d in config
                .telegram
                .destinations
                .iter()
                .filter(|d| d.bot_id == bot && d.chat_id == message.chat_id)
            {
                if let Some(s) = self
                    .ledger
                    .connector_stream_for_platform(&c.id, &d.id, &topic.to_string())
                    .map_err(|_| "ledger_unavailable")?
                {
                    if s.provider_scope != format!("{bot}:{}", message.chat_id) {
                        continue;
                    }
                    let now = now_ms();
                    return Ok(Some(ConnectorInboundRow {
                        id: stable_connector_id("inbound", &[&c.id, &d.id, &mid.to_string()]),
                        stream_id: s.id,
                        provider_message_id: mid.to_string(),
                        return_host_id: s.return_host_id,
                        body: Some(message.text.clone()),
                        content_digest: format!("{:x}", Sha256::digest(message.text.as_bytes())),
                        status: "received".into(),
                        created_at_ms: now,
                        expires_at_ms: now.saturating_add(86400000),
                    }));
                }
            }
        }
        tracing::warn!("Telegram topic input rejected: no matching ready stream");
        Ok(None)
    }
    pub(super) async fn deliver_telegram(
        &self,
        client: &TelegramBotApiClient,
        bot: i64,
        name: &str,
    ) -> Result<(), &'static str> {
        let settings = self
            .settings
            .load_message_connectors()
            .map_err(|_| "settings_unavailable")?;
        for connector in settings.connectors.iter().filter(|c| {
            c.enabled
                && c.host_id == self.host_id
                && c.connection_ref.is_none()
                && c.provider == ConnectorProviderKind::Telegram
        }) {
            for row in self
                .ledger
                .pending_connector_deliveries(&connector.id, now_ms())
                .map_err(|_| "ledger_unavailable")?
            {
                let Some(mut stream) = self
                    .ledger
                    .connector_stream(&row.stream_id)
                    .map_err(|_| "ledger_unavailable")?
                else {
                    continue;
                };
                let config = self
                    .settings
                    .load_codex_control()
                    .map_err(|_| "settings_unavailable")?;
                let destination =
                    config.telegram.destinations.iter().find(|d| {
                        d.id == stream.destination_ref && d.bot_id == bot && d.chat_id > 0
                    });
                let Some(destination) = destination.filter(|_| config.telegram.enabled) else {
                    self.ledger
                        .transition_connector_delivery(
                            &row.id,
                            "pending",
                            "failed",
                            None,
                            Some("destination_unavailable".into()),
                            now_ms(),
                        )
                        .map_err(|_| "ledger_unavailable")?;
                    self.publisher.work(super::application_state::DeliveryChannel::Connector);
                    self.publisher
                        .publish(ApplicationStateApiV1::NotificationV1);
                    continue;
                };
                if stream.provider_scope != format!("{bot}:{}", destination.chat_id) {
                    self.ledger
                        .transition_connector_delivery(
                            &row.id,
                            "pending",
                            "failed",
                            None,
                            Some("provider_account_changed".into()),
                            now_ms(),
                        )
                        .map_err(|_| "ledger_unavailable")?;
                    self.publisher.work(super::application_state::DeliveryChannel::Connector);
                    self.publisher
                        .publish(ApplicationStateApiV1::NotificationV1);
                    continue;
                }
                if stream.status == "pending" {
                    // Query support before making a non-idempotent topic creation request.
                    if !client
                        .has_private_topics(name)
                        .await
                        .map_err(|_| "telegram_capability_unavailable")?
                    {
                        self.ledger
                            .transition_connector_delivery(
                                &row.id,
                                "pending",
                                "failed",
                                None,
                                Some("private_topics_disabled".into()),
                                now_ms(),
                            )
                            .map_err(|_| "ledger_unavailable")?;
                        self.publisher
                            .publish(ApplicationStateApiV1::NotificationV1);
                        continue;
                    }
                    if !self
                        .ledger
                        .claim_connector_stream(&stream.id)
                        .map_err(|_| "ledger_unavailable")?
                    {
                        continue;
                    }
                    let title: String = format!(
                        "{} · {}",
                        if stream.title.is_empty() {
                            "Codex"
                        } else {
                            &stream.title
                        },
                        stream.thread_id
                    )
                    .chars()
                    .take(128)
                    .collect();
                    let created = client
                        .create_forum_topic(name, destination.chat_id, &title)
                        .await;
                    let (status, platform, error) = match created {
                        Ok(id) => ("ready", Some(id.to_string()), None),
                        Err(e) => {
                            let status = definite_failure(&e);
                            (status, None, Some("topic_creation_failed".into()))
                        }
                    };
                    self.ledger
                        .finish_connector_stream(&stream.id, status, platform, error)
                        .map_err(|_| "ledger_unavailable")?;
                    stream = self
                        .ledger
                        .connector_stream(&stream.id)
                        .map_err(|_| "ledger_unavailable")?
                        .ok_or("stream_not_found")?;
                }
                if matches!(stream.status.as_str(), "failed" | "unknown") {
                    self.ledger
                        .transition_connector_delivery(
                            &row.id,
                            "pending",
                            &stream.status,
                            None,
                            stream.error_code,
                            now_ms(),
                        )
                        .map_err(|_| "ledger_unavailable")?;
                    self.publisher.work(super::application_state::DeliveryChannel::Connector);
                    self.publisher
                        .publish(ApplicationStateApiV1::NotificationV1);
                    continue;
                }
                if stream.status != "ready" {
                    continue;
                }
                let topic = stream
                    .platform_id
                    .as_deref()
                    .and_then(|id| id.parse::<i64>().ok())
                    .ok_or("invalid_topic")?;
                let text = format!(
                    "Polar Station · {}\n会话：{}\nID：{}\n\n{}",
                    row.event_kind,
                    stream.title,
                    stream.thread_id,
                    row.body.as_deref().ok_or("body_expired")?
                );
                if text.encode_utf16().count() > 4096 {
                    self.ledger
                        .transition_connector_delivery(
                            &row.id,
                            "pending",
                            "failed",
                            None,
                            Some("message_too_long".into()),
                            now_ms(),
                        )
                        .map_err(|_| "ledger_unavailable")?;
                    self.publisher.work(super::application_state::DeliveryChannel::Connector);
                    self.publisher
                        .publish(ApplicationStateApiV1::NotificationV1);
                    continue;
                }
                if !self
                    .ledger
                    .transition_connector_delivery(
                        &row.id,
                        "pending",
                        "sending",
                        None,
                        None,
                        now_ms(),
                    )
                    .map_err(|_| "ledger_unavailable")?
                {
                    continue;
                }
                let sent = client
                    .send_text_in_thread(
                        TelegramOutboundMessage {
                            bot_name: name.into(),
                            chat_id: destination.chat_id,
                            text,
                        },
                        topic,
                        None,
                    )
                    .await;
                let (status, id, error) = match sent {
                    Ok(sent) => ("sent", Some(sent.message_id.to_string()), None),
                    Err(e) => (definite_failure(&e), None, Some("topic_send_failed".into())),
                };
                self.ledger
                    .transition_connector_delivery(&row.id, "sending", status, id, error, now_ms())
                    .map_err(|_| "ledger_unavailable")?;
                self.publisher.work(super::application_state::DeliveryChannel::Connector);
                self.publisher
                    .publish(ApplicationStateApiV1::NotificationV1);
            }
        }
        Ok(())
    }
}
fn definite_failure(error: &anyhow::Error) -> &'static str {
    match error.downcast_ref::<TelegramApiFailure>() {
        Some(TelegramApiFailure::Rejected { code }) if *code < 500 => "failed",
        Some(
            TelegramApiFailure::Unauthorized
            | TelegramApiFailure::RateLimited { .. }
            | TelegramApiFailure::ConsumerConflict,
        ) => "failed",
        _ => "unknown",
    }
}
