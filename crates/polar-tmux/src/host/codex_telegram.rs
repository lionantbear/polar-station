//! Telegram reply admission belongs to the execution Host, using its existing ledger.
use super::codex_control::{CodexControlService, ControlResult};
use super::imessage_sender::now_ms;
use polar_station_settings::TelegramControlSettingsV1;
use polar_station_store_sqlite::{CodexSubmissionRow, SubmissionTransition};
use polar_telegram::{TelegramInboundMessage, TelegramProviderUpdate, TelegramUpdateBatch};
use polar_tmux_protocol::{validate_codex_input, ApplicationStateApiV1, CodexCommandV1};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

impl CodexControlService {
    pub(super) fn telegram_row_authorized(
        &self,
        settings: &TelegramControlSettingsV1,
        row: &CodexSubmissionRow,
    ) -> bool {
        let (Some(bot), Some(chat)) = (row.bot_id, row.chat_id) else {
            return false;
        };
        let sender = row
            .principal
            .rsplit(':')
            .next()
            .and_then(|s| s.parse::<i64>().ok());
        allowed(settings, bot, chat, sender)
            && row.principal == principal(bot, chat, sender.unwrap_or(0))
    }

    pub(super) async fn resume_received_telegram(&self, bot: i64) -> ControlResult<()> {
        for row in self
            .ledger
            .received_telegram_submissions(bot)
            .map_err(|_| "ledger_unavailable")?
        {
            let _gate = self.operations.lock().await;
            // A restarted observer may not have rejoined yet. Keep the original
            // received row until the connection is usable or its deadline expires.
            if row.expires_at_ms > now_ms() {
                let states = self.states.read().await;
                if row
                    .binding_id
                    .as_ref()
                    .and_then(|id| states.get(id))
                    .is_some_and(|s| {
                        matches!(
                            s.connection,
                            polar_tmux_protocol::CodexConnectionStateV1::Disconnected
                                | polar_tmux_protocol::CodexConnectionStateV1::Connecting
                        )
                    })
                {
                    continue;
                }
            }
            self.submit_received_telegram(&row).await?;
        }
        Ok(())
    }

    /// Commit every accepted/rejected update and the cursor before any RPC.
    pub(super) async fn receive_telegram(
        &self,
        bot_id: i64,
        batch: TelegramUpdateBatch,
    ) -> ControlResult<Vec<CodexSubmissionRow>> {
        if bot_id <= 0 || batch.updates.len() > 100 {
            return Err("invalid_telegram_batch");
        }
        let Some(offset) = batch.next_offset else {
            return if batch.updates.is_empty() {
                Ok(vec![])
            } else {
                Err("invalid_telegram_cursor")
            };
        };
        let offset = i64::try_from(offset).map_err(|_| "invalid_telegram_cursor")?;
        if batch.updates.iter().any(|u| u.update_id >= offset as u64) {
            return Err("invalid_telegram_cursor");
        }
        let mut ids = Vec::new();
        {
            let _operation = self.operations.lock().await;
            let config = self.config.read().await.clone();
            if !config.telegram.enabled {
                return Err("telegram_disabled");
            }
            let mut rows = Vec::new();
            let mut topics = Vec::new();
            let mut seen = BTreeSet::new();
            for update in batch.updates {
                if let Some(message) = update
                    .message
                    .as_ref()
                    .filter(|m| m.metadata.message_thread_id.is_some())
                {
                    if let Some(row) = self.connector_service().telegram_inbound(bot_id, message)? {
                        topics.push(row);
                    }
                    continue;
                }
                let row = self
                    .telegram_row(bot_id, &batch.bot_name, &config.telegram, update)
                    .await?;
                if seen.insert(row.intent_id.clone()) {
                    ids.push(row.intent_id.clone());
                }
                rows.push(row);
            }
            self.ledger
                .accept_connector_telegram_batch(bot_id, offset, now_ms(), topics, rows)
                .map_err(|_| "ledger_unavailable")?;
        }
        let mut changed = Vec::new();
        for id in ids {
            // Revocation can win between any two deliveries; do not hold this
            // gate across an entire batch of network writes.
            let _operation = self.operations.lock().await;
            let Some(row) = self
                .ledger
                .submission(&id)
                .map_err(|_| "ledger_unavailable")?
            else {
                continue;
            };
            if row.status != "received" {
                continue;
            }
            self.submit_received_telegram(&row).await?;
            if let Some(receipt) = self
                .ledger
                .submission(&id)
                .map_err(|_| "ledger_unavailable")?
            {
                changed.push(receipt);
            }
        }
        self.publisher.work(super::application_state::DeliveryChannel::Telegram);
        self.publisher.work(super::application_state::DeliveryChannel::Connector);
        self.publisher.publish(ApplicationStateApiV1::CodexV1);
        self.publisher
            .publish(ApplicationStateApiV1::NotificationV1);
        Ok(changed)
    }

    async fn telegram_row(
        &self,
        bot_id: i64,
        bot_name: &str,
        settings: &TelegramControlSettingsV1,
        update: TelegramProviderUpdate,
    ) -> ControlResult<CodexSubmissionRow> {
        let update_id = i64::try_from(update.update_id).map_err(|_| "invalid_telegram_cursor")?;
        let message = update.message.as_ref();
        let chat = message.map(|m| m.chat_id);
        let sender = message.and_then(|m| m.remote_user_id);
        let message_id = message.and_then(|m| m.metadata.message_id).map(i64::from);
        let intent_id = match (chat, message_id) {
            (Some(chat), Some(message)) => format!("telegram-{bot_id}-{chat}-{message}"),
            _ => format!("telegram-update-{bot_id}-{update_id}"),
        };
        let now = now_ms();
        let mut row = CodexSubmissionRow {
            intent_id: intent_id.clone(),
            principal: principal(bot_id, chat.unwrap_or(0), sender.unwrap_or(0)),
            binding_id: None,
            binding_revision: None,
            operation_kind: "reject".into(),
            expected_turn_id: None,
            source_kind: "telegram".into(),
            bot_id: Some(bot_id),
            chat_id: chat,
            message_id,
            update_id: Some(update_id),
            client_message_id: None,
            status: "rejected".into(),
            queue_id: None,
            turn_id: None,
            input_text: None,
            input_digest: format!(
                "{:x}",
                Sha256::digest(message.map_or("", |m| &m.text).as_bytes())
            ),
            created_at_ms: now,
            updated_at_ms: now,
            expires_at_ms: now + 15 * 60 * 1000,
            error_code: Some("unsupported_update".into()),
        };
        let Some(message) = message else {
            return Ok(row);
        };
        let result = async {
            if !allowed(settings, bot_id, message.chat_id, message.remote_user_id) {
                return Err("source_denied");
            }
            if message.bot_name != bot_name || !ordinary_text(message) {
                return Err("unsupported_message");
            }
            let reply = message
                .metadata
                .reply_to_message_id
                .ok_or("notification_unknown")?;
            let notice = self
                .ledger
                .telegram_notification(bot_id, message.chat_id, i64::from(reply))
                .map_err(|_| "ledger_unavailable")?
                .ok_or("notification_unknown")?;
            if notice.expires_at_ms <= now {
                return Err("notification_expired");
            }
            let revision =
                u64::try_from(notice.binding_revision).map_err(|_| "binding_conflict")?;
            let state = self.current_binding(&notice.binding_id, revision).await?;
            if !state.can_queue {
                return Err("control_unavailable");
            }
            let command = CodexCommandV1::QueueText {
                binding_id: notice.binding_id.clone(),
                expected_revision: revision,
                intent_id: intent_id.clone(),
                text: message.text.clone(),
            };
            row.input_digest = format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&command).map_err(|_| "invalid_request")?)
            );
            row.binding_id = Some(notice.binding_id);
            row.binding_revision = Some(notice.binding_revision);
            row.operation_kind = "queue".into();
            row.client_message_id = Some(intent_id);
            row.status = "received".into();
            row.input_text = Some(message.text.clone());
            row.expires_at_ms = row.expires_at_ms.min(notice.expires_at_ms);
            Ok(())
        }
        .await;
        // A storage failure cannot acknowledge a batch as a business rejection.
        if result == Err("ledger_unavailable") {
            return Err("ledger_unavailable");
        }
        row.error_code = result.err().map(str::to_owned);
        Ok(row)
    }

    async fn submit_received_telegram(&self, row: &CodexSubmissionRow) -> ControlResult<()> {
        let result = async {
            let config = self.config.read().await;
            let bot = row.bot_id.ok_or("source_denied")?;
            let chat = row.chat_id.ok_or("source_denied")?;
            let sender = row
                .principal
                .rsplit(':')
                .next()
                .and_then(|id| id.parse::<i64>().ok());
            if !config.enabled
                || !config.telegram.enabled
                || !allowed(&config.telegram, bot, chat, sender)
                || row.principal != principal(bot, chat, sender.unwrap_or(0))
            {
                return Err("source_denied");
            }
            drop(config);
            let command = CodexCommandV1::QueueText {
                binding_id: row.binding_id.clone().ok_or("binding_not_found")?,
                expected_revision: row
                    .binding_revision
                    .and_then(|r| u64::try_from(r).ok())
                    .ok_or("binding_conflict")?,
                intent_id: row.intent_id.clone(),
                text: row.input_text.clone().ok_or("expired")?,
            };
            self.submit(&row.principal, command).await.map(|_| ())
        }
        .await;
        if let Err(code) = result {
            if code == "ledger_unavailable" {
                return Err(code);
            }
            self.ledger
                .transition_submission(
                    &row.intent_id,
                    "received",
                    SubmissionTransition {
                        status: "rejected".into(),
                        queue_id: None,
                        turn_id: None,
                        error_code: Some(code.into()),
                        at_ms: now_ms(),
                    },
                )
                .map_err(|_| "ledger_unavailable")?;
        }
        Ok(())
    }
}

fn principal(bot: i64, chat: i64, sender: i64) -> String {
    format!("telegram:{bot}:{chat}:{sender}")
}

fn allowed(settings: &TelegramControlSettingsV1, bot: i64, chat: i64, sender: Option<i64>) -> bool {
    settings
        .allowed_sources
        .iter()
        .any(|s| s.bot_id == bot && s.chat_id == chat && Some(s.sender_id) == sender)
}

fn ordinary_text(message: &TelegramInboundMessage) -> bool {
    message.metadata.message_id.is_some_and(|id| id > 0)
        && matches!(
            message.metadata.chat_type.as_deref(),
            Some("private" | "group" | "supergroup")
        )
        && !message.metadata.edited
        && !message.metadata.forwarded
        && message.metadata.message_thread_id.is_none()
        && message.attachments.is_empty()
        && validate_codex_input(&message.text).is_ok()
}
