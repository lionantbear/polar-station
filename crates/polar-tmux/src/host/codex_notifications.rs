//! Completion facts and immutable channel destinations; no provider I/O in the event reader.
use super::codex_control::{apply_turn, CodexControlService, ControlResult};
use super::codex_endpoint::{CodexEndpoint, RpcFailure};
use super::imessage_sender::now_ms;
use polar_station_inbox::{CodexCompletionOrigin, CodexCompletionStatus, InboxMessageId};
use polar_station_store_sqlite::{CodexCheckpointRow, CodexNotificationRow};
use polar_tmux_protocol::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

impl CodexControlService {
    pub(super) async fn reconcile_history(
        &self,
        state: &mut CodexStateV1,
        client: &CodexEndpoint,
    ) -> ControlResult<()> {
        let started = self.observations.lock().unwrap().get(&state.binding.binding_id).is_some_and(|o| o.started);
        let binding = self.persist_binding(&state.binding.binding_id)?;
        if started && self.ledger.checkpoint(&binding.id).map_err(|_| "ledger_unavailable")?.is_none() {
            self.ledger.record_completion(CodexCheckpointRow {
                binding_id: binding.id.clone(), binding_revision: binding.revision,
                last_reconciled_turn: None, bound_at_ms: binding.bound_at_ms,
                reconcile_status: "current".into(),
            }, vec![]).map_err(|_| "ledger_unavailable")?;
        }
        let checkpoint = self
            .ledger
            .checkpoint(&binding.id)
            .map_err(|_| "ledger_unavailable")?;
        let initial = checkpoint
            .as_ref()
            .is_none_or(|c| c.bound_at_ms != binding.bound_at_ms);
        let watermark = checkpoint
            .as_ref()
            .and_then(|c| c.last_reconciled_turn.as_deref());
        let mut cursor: Option<String> = None;
        let mut scanned = 0;
        let mut completed = Vec::new();
        let mut found_boundary = initial;
        let mut more;
        loop {
            let page = client
                .request(
                    "thread/turns/list",
                    json!({
                        "threadId":binding.thread_id,"limit":50,"cursor":cursor,
                        "sortDirection":"desc","itemsView":"summary",
                    }),
                )
                .await
                .map_err(RpcFailure::code)?;
            let turns = page["data"].as_array().ok_or("invalid_codex_response")?;
            if turns.len() > 50 {
                return Err("invalid_codex_response");
            }
            if scanned == 0 {
                if let Some(turn) = turns.first() {
                    apply_turn(state, turn);
                }
            }
            for turn in turns {
                scanned += 1;
                let id = turn["id"].as_str().ok_or("invalid_codex_response")?;
                if !initial && Some(id) == watermark {
                    found_boundary = true;
                    break;
                }
                if !terminal(turn) {
                    continue;
                }
                // When binding an empty/active conversation there is no terminal
                // anchor yet. Do not include completions older than the binding.
                if !initial
                    && watermark.is_none()
                    && turn["completedAt"]
                        .as_i64()
                        .is_some_and(|t| t < binding.bound_at_ms / 1000)
                {
                    found_boundary = true;
                    break;
                }
                completed.push(turn.clone());
            }
            let next = page["nextCursor"].as_str().map(str::to_owned);
            more = next.is_some();
            if initial || found_boundary || !more || scanned >= 200 {
                break;
            }
            if next == cursor || turns.is_empty() {
                return Err("invalid_codex_response");
            }
            cursor = next;
        }
        if initial {
            // Establish a baseline; binding is never a request to send old results.
            self.ledger
                .record_completion(
                    CodexCheckpointRow {
                        binding_id: binding.id,
                        binding_revision: binding.revision,
                        last_reconciled_turn: completed
                            .first()
                            .and_then(|t| t["id"].as_str())
                            .map(str::to_owned),
                        bound_at_ms: binding.bound_at_ms,
                        reconcile_status: "current".into(),
                    },
                    vec![],
                )
                .map_err(|_| "ledger_unavailable")?;
            return Ok(());
        }
        if !found_boundary && (more || watermark.is_some()) {
            let code = if more {
                "history_backlog"
            } else {
                "history_gap"
            };
            state.error_code = Some(code.into());
            let mut checkpoint = checkpoint.ok_or("ledger_unavailable")?;
            checkpoint.reconcile_status = code.into();
            self.ledger
                .record_completion(checkpoint, vec![])
                .map_err(|_| "ledger_unavailable")?;
            return Ok(());
        }
        // Oldest first: a crash can resume from the last fully committed turn.
        for turn in completed.iter().rev() {
            let mut recovered = state.clone();
            recovered.turn_id = None;
            recovered.final_answer = None;
            apply_turn(&mut recovered, turn);
            self.record_turn_completion(&recovered, true).await?;
        }
        Ok(())
    }

    pub(super) async fn record_turn_completion(
        &self,
        state: &CodexStateV1,
        reconciled: bool,
    ) -> ControlResult<()> {
        let Some(turn_id) = state.turn_id.as_ref() else {
            return Err("invalid_codex_response");
        };
        if !matches!(
            state.activity,
            CodexActivityV1::Completed | CodexActivityV1::Failed | CodexActivityV1::Interrupted
        ) {
            return Ok(());
        }
        let binding = self.binding_row(&state.binding.binding_id)?;
        let revision = state
            .binding
            .revision
            .try_into()
            .map_err(|_| "invalid_revision")?;
        if binding.revision != revision {
            return Err("binding_conflict");
        }
        let config = self.config.read().await;
        if !config.enabled {
            return Err("control_disabled");
        }
        let now = now_ms();
        let session_name = if let Some(key) = &state.binding.session {
            self.sessions
                .snapshot()
                .await
                .sessions
                .into_iter()
                .find(|s| &s.key == key)
                .map(|s| s.display_name)
        } else {
            None
        };
        let target = config
            .endpoints
            .iter()
            .find(|e| e.id == state.binding.endpoint_id)
            .ok_or("endpoint_not_found")?
            .execution_target_id
            .clone();
        let body = completion_body(state, session_name.as_deref(), &target);
        let notice = |channel: &str, destination_key: String, body: String| CodexNotificationRow {
            id: format!(
                "notification-{:x}",
                Sha256::digest(
                    serde_json::to_vec(&(
                        &binding.id,
                        revision,
                        turn_id,
                        channel,
                        &destination_key,
                    ))
                    .expect("notification identity serialization")
                )
            ),
            binding_id: binding.id.clone(),
            binding_revision: revision,
            turn_id: turn_id.clone(),
            channel: channel.into(),
            destination_key,
            bot_id: None,
            chat_id: None,
            sender_host_id: None,
            sender_generation: None,
            status: "pending".into(),
            provider_message_id: None,
            body_digest: format!("{:x}", Sha256::digest(body.as_bytes())),
            bounded_body: Some(body),
            created_at_ms: now,
            expires_at_ms: now.saturating_add(24 * 60 * 60 * 1000),
            error_code: None,
        };
        let connector_settings = self
            .settings
            .load_message_connectors()
            .map_err(|_| "settings_unavailable")?;
        let routes = connector_settings.routes_for(&binding.endpoint_id, &binding.thread_id);
        let routed = |provider| {
            connector_settings
                .routes
                .iter()
                .filter(|r| {
                    r.endpoint_id == binding.endpoint_id
                        && r.thread_id
                            .as_deref()
                            .is_none_or(|id| id == binding.thread_id)
                })
                .any(|r| {
                    connector_settings
                        .connectors
                        .iter()
                        .any(|c| c.id == r.connector_id && c.provider == provider)
                })
        };
        let mut notifications = Vec::new();
        for route in &routes {
            let Some(connector) = connector_settings
                .connectors
                .iter()
                .find(|c| c.id == route.connector_id && c.enabled)
            else {
                continue;
            };
            let publication = ConnectorPublicationV1 {
                connector_id: connector.id.clone(),
                destination_ref: route.destination_ref.clone(),
                endpoint_id: binding.endpoint_id.clone(),
                thread_id: binding.thread_id.clone(),
                turn_id: turn_id.clone(),
                event_kind: match state.activity {
                    CodexActivityV1::Completed => "completed",
                    CodexActivityV1::Failed => "failed",
                    _ => "interrupted",
                }
                .into(),
                thread_name: state.thread_name.clone().unwrap_or_else(|| "未命名".into()),
                text: bounded_text(
                    state
                        .final_answer
                        .as_deref()
                        .unwrap_or("请在原会话查看本轮结果。"),
                    3500,
                ),
            };
            let mut row = notice(
                super::connector_monitor::connector_channel(connector.provider),
                format!("connector:{}", publication.stream_id()),
                serde_json::to_string(&publication).map_err(|_| "invalid_publication")?,
            );
            row.sender_host_id = Some(connector.host_id.clone());
            notifications.push(row);
        }
        if config.telegram.enabled
            && !routed(polar_station_settings::ConnectorProviderKind::Telegram)
        {
            if let Some(destination) = config
                .telegram
                .destinations
                .iter()
                .find(|d| Some(&d.id) == binding.telegram_destination_ref.as_ref())
            {
                // Snapshot provider identity as well as the human-facing reference.
                let key = serde_json::to_string(destination).map_err(|_| "invalid_destination")?;
                let mut row = notice("telegram", key, body.clone());
                row.bot_id = Some(destination.bot_id);
                row.chat_id = Some(destination.chat_id);
                notifications.push(row);
            }
        }
        if config.imessage_enabled
            && binding.imessage_enabled
            && !routed(polar_station_settings::ConnectorProviderKind::Imessage)
        {
            if let Some(destination) = &config.imessage_sender {
                let key = serde_json::to_string(destination).map_err(|_| "invalid_destination")?;
                let mut text: String = body.chars().take(1200).collect();
                if body.chars().count() > 1200 {
                    text.pop();
                    text.push('…');
                }
                let mut row = notice("imessage", key, text);
                row.sender_host_id = Some(destination.host_id.clone());
                row.sender_generation = Some(destination.sender_generation.clone());
                notifications.push(row);
            }
        }
        let mut checkpoint = CodexCheckpointRow {
            binding_id: binding.id,
            binding_revision: revision,
            last_reconciled_turn: Some(turn_id.clone()),
            bound_at_ms: binding.bound_at_ms,
            reconcile_status: "current".into(),
        };
        if !reconciled {
            if let Some(previous) = self
                .ledger
                .checkpoint(&checkpoint.binding_id)
                .map_err(|_| "ledger_unavailable")?
            {
                if previous.bound_at_ms == checkpoint.bound_at_ms
                    && previous.reconcile_status != "current"
                {
                    // A new live result can be delivered, but cannot certify an
                    // unobserved interval or move its recovery watermark.
                    checkpoint = previous;
                }
            }
        }
        let id = format!(
            "codex-{:x}",
            Sha256::digest(
                serde_json::to_vec(&(
                    &state.binding.owner_host_id,
                    &state.binding.endpoint_id,
                    &state.binding.thread_id,
                    turn_id,
                ))
                .expect("completion identity")
            )
        );
        let mut summary = body;
        if summary.len() > 4096 {
            let mut end = 4093;
            while !summary.is_char_boundary(end) {
                end -= 1;
            }
            summary.truncate(end);
            summary.push('…');
        }
        let completion = CodexCompletion {
            id: InboxMessageId::new(id).map_err(|_| "invalid_completion")?,
            origin: CodexCompletionOrigin {
                owner_host_id: state.binding.owner_host_id.clone(),
                endpoint_id: state.binding.endpoint_id.clone(),
                thread_id: state.binding.thread_id.clone(),
                turn_id: turn_id.clone(),
                binding_id: state.binding.binding_id.clone(),
                binding_revision: state.binding.revision,
                execution_target_id: target,
                tmux_session_id: state
                    .binding
                    .session
                    .as_ref()
                    .map(|s| s.tmux_session_id.clone()),
            },
            status: match state.activity {
                CodexActivityV1::Completed => CodexCompletionStatus::Completed,
                CodexActivityV1::Failed => CodexCompletionStatus::Failed,
                CodexActivityV1::Interrupted => CodexCompletionStatus::Interrupted,
                _ => return Err("invalid_completion"),
            },
            summary: summary.trim().into(),
            created_at_ms: now.max(0) as u64,
        };
        use super::application_state::DeliveryChannel;
        let channels = notifications.iter().map(|row| {
            if row.destination_key.starts_with("connector:") { DeliveryChannel::Connector }
            else if row.channel == "telegram" { DeliveryChannel::Telegram }
            else { DeliveryChannel::Imessage }
        }).collect::<std::collections::BTreeSet<_>>();
        self.ledger
            .record_completion_with_inbox(checkpoint, notifications, Some(completion))
            .map_err(|_| "ledger_unavailable")?;
        for channel in channels {
            self.publisher.work(channel);
        }
        Ok(())
    }

    pub(super) fn completion_page(
        &self,
        endpoint: Option<&str>,
        before: Option<CodexCompletionCursor>,
        limit: u16,
    ) -> ControlResult<CodexResultV1> {
        let mut records = self
            .ledger
            .completion_page_with_notifications(before, limit as usize + 1, &self.owner, endpoint)
            .map_err(|_| "ledger_unavailable")?;
        let more = records.len() > limit as usize;
        records.truncate(limit as usize);
        let next_before = if more {
            records.last().map(|r| CodexCompletionCursor {
                created_at_ms: r.completion.created_at_ms,
                id: r.completion.id.clone(),
            })
        } else {
            None
        };
        let completions = records
            .into_iter()
            .map(|record| {
                let completion = record.completion;
                let deliveries = record
                    .notifications
                    .into_iter()
                    .map(|row| {
                        let (status, label) = match (row.channel.as_str(), row.status.as_str()) {
                            ("imessage", "sent") => (
                                "sent_to_messages",
                                "Accepted by Messages; delivery is not confirmed",
                            ),
                            (_, "sent") => ("sent", "Sent"),
                            (_, "pending") => ("pending", "Waiting to send"),
                            (_, "sending") => ("sending", "Sending"),
                            (_, "awaiting_receipt") => {
                                ("awaiting_receipt", "Checking original send receipt")
                            }
                            (_, "failed") => ("failed", "Send failed"),
                            (_, "expired") => ("expired", "Expired"),
                            _ => ("unknown", "Send result unknown; no automatic resend"),
                        };
                        CodexDeliveryProjectionV1 {
                            channel: row.channel,
                            status: status.into(),
                            label: label.into(),
                            created_at_ms: row.created_at_ms.max(0) as u64,
                            error_code: row.error_code,
                        }
                    })
                    .collect();
                CodexCompletionProjectionV1 {
                    completion,
                    deliveries,
                }
            })
            .collect();
        Ok(CodexResultV1::Completions {
            completions,
            next_before,
        })
    }
}

fn terminal(turn: &Value) -> bool {
    matches!(
        turn["status"].as_str(),
        Some("completed" | "failed" | "interrupted")
    )
}

fn completion_body(state: &CodexStateV1, session_name: Option<&str>, target: &str) -> String {
    let (status, answer) = match state.activity {
        CodexActivityV1::Completed => (
            "已完成",
            state
                .final_answer
                .as_deref()
                .unwrap_or("本轮已完成，未取得最终答复。"),
        ),
        CodexActivityV1::Failed => ("失败", "本轮执行失败，请在原会话查看详情。"),
        CodexActivityV1::Interrupted => ("已中断", "本轮执行已中断。"),
        _ => ("状态未知", "请在原会话查看详情。"),
    };
    let name = state.thread_name.as_deref().unwrap_or("未命名");
    let terminal = state
        .binding
        .session
        .as_ref()
        .map(|session| {
            let name: String = session_name
                .unwrap_or("未命名")
                .chars()
                .filter(|c| !c.is_control())
                .take(120)
                .collect();
            format!("\ntmux：{name}（{}）", session.tmux_session_id)
        })
        .unwrap_or_default();
    bounded_text(
        &format!(
            "Polar Station · {status}\n会话：{name}\nID：{}\nHost：{target}{terminal}\n\n{answer}",
            state.binding.thread_id
        ),
        3500,
    )
}

/// Telegram counts UTF-16 units. Preserve Unicode scalar boundaries and mark truncation.
pub(super) fn bounded_text(text: &str, max_units: usize) -> String {
    let mut result = String::new();
    let mut units = 0;
    for ch in text
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
    {
        if units + ch.len_utf16() > max_units {
            while units >= max_units {
                let Some(last) = result.pop() else {
                    return result;
                };
                units -= last.len_utf16();
            }
            result.push('…');
            return result;
        }
        units += ch.len_utf16();
        result.push(ch);
    }
    result
}
