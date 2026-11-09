//! Durable submission receipts around Codex's existing queue and turn operations.
use super::codex_control::{CodexControlService, ControlResult};
use super::codex_endpoint::RpcFailure;
use super::imessage_sender::now_ms;
use polar_station_store_sqlite::{CodexSubmissionRow, SubmissionTransition};
use polar_tmux_protocol::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

impl CodexControlService {
    // An explicit sender reference is durable for receipt lookup, but does not
    // populate the runtime observation catalog or inherit notification defaults.
    async fn sender_binding(
        &self,
        endpoint: &str,
        thread: &str,
    ) -> ControlResult<polar_station_store_sqlite::CodexBindingRow> {
        let observed = self
            .states
            .read()
            .await
            .values()
            .find(|s| s.binding.endpoint_id == endpoint && s.binding.thread_id == thread)
            .map(|s| s.binding.binding_id.clone());
        if let Some(id) = observed {
            return self.persist_binding(&id);
        }
        let digest = Sha256::digest(
            serde_json::to_vec(&(&self.owner, endpoint, thread)).map_err(|_| "invalid_thread")?,
        );
        let id = format!("codex-{digest:x}");
        if let Some(row) = self.ledger.binding(&id).map_err(|_| "ledger_unavailable")? {
            if !row.enabled {
                return Err("thread_released");
            }
            return Ok(row);
        }
        let row = polar_station_store_sqlite::CodexBindingRow {
            id,
            owner_host_id: self.owner.clone(),
            revision: 1,
            session_key: "null".into(),
            endpoint_id: endpoint.into(),
            thread_id: thread.into(),
            enabled: true,
            telegram_destination_ref: None,
            imessage_enabled: false,
            bound_at_ms: now_ms(),
        };
        if !self
            .ledger
            .save_binding(0, row.clone())
            .map_err(|_| "ledger_unavailable")?
        {
            return Err("binding_conflict");
        }
        Ok(row)
    }

    pub(super) async fn reconcile_receipt(
        &self,
        principal: &str,
        id: &str,
    ) -> ControlResult<CodexResultV1> {
        let existing = self.receipt(principal, id)?;
        let Some(row) = self
            .ledger
            .submission(id)
            .map_err(|_| "ledger_unavailable")?
        else {
            return Ok(existing);
        };
        if row.status != "unknown" || row.operation_kind == "interrupt" {
            return Ok(existing);
        }
        let Some(binding_id) = row.binding_id.as_ref() else {
            return Ok(existing);
        };
        let Ok(binding) = self.binding_row(binding_id) else {
            return Ok(existing);
        };
        if Some(binding.revision) != row.binding_revision {
            return Ok(existing);
        }
        let Ok(client) = self.endpoint(&binding.endpoint_id).await else {
            return Ok(existing);
        };
        let mut confirmed = None;
        // Queue and history are evidence only. An absent client ID cannot prove
        // that the request never executed, so it never authorizes resubmission.
        for method in ["thread/queue/list", "thread/items/list"] {
            if method == "thread/queue/list" && row.operation_kind != "queue" {
                continue;
            }
            let mut cursor: Option<String> = None;
            for _ in 0..4 {
                let params = if method == "thread/queue/list" {
                    json!({"threadId":binding.thread_id,"limit":50,"cursor":cursor})
                } else {
                    json!({"threadId":binding.thread_id,"turnId":row.expected_turn_id,"limit":50,"cursor":cursor,"sortDirection":"desc"})
                };
                let Ok(page) = client.request(method, params).await else {
                    break;
                };
                let Some(items) = page["data"].as_array().filter(|v| v.len() <= 50) else {
                    break;
                };
                for item in items {
                    if method == "thread/queue/list"
                        && item["clientUserMessageId"].as_str() == row.client_message_id.as_deref()
                    {
                        if let Some(queue_id) = item["id"].as_str() {
                            confirmed = Some((Some(queue_id.into()), None));
                        }
                    } else if method == "thread/items/list"
                        && item["item"]["type"] == "userMessage"
                        && item["item"]["clientId"].as_str() == row.client_message_id.as_deref()
                    {
                        if let Some(turn_id) = item["turnId"].as_str() {
                            if row
                                .expected_turn_id
                                .as_deref()
                                .is_none_or(|expected| expected == turn_id)
                            {
                                confirmed = Some((None, Some(turn_id.into())));
                            }
                        }
                    }
                }
                if confirmed.is_some() {
                    break;
                }
                let next = page["nextCursor"].as_str().map(str::to_owned);
                if next.is_none() || next == cursor || items.is_empty() {
                    break;
                }
                cursor = next;
            }
            if confirmed.is_some() {
                break;
            }
        }
        if let Some((queue_id, turn_id)) = confirmed {
            let changed = self
                .ledger
                .transition_submission(
                    id,
                    "unknown",
                    SubmissionTransition {
                        status: "accepted".into(),
                        queue_id,
                        turn_id,
                        error_code: None,
                        at_ms: now_ms(),
                    },
                )
                .map_err(|_| "ledger_unavailable")?;
            if changed {
                self.publisher
                    .work(super::application_state::DeliveryChannel::Telegram);
                self.publisher.publish(ApplicationStateApiV1::CodexV1);
            }
        }
        self.receipt(principal, id)
    }

    pub(super) fn receipt(&self, principal: &str, id: &str) -> ControlResult<CodexResultV1> {
        let row = self
            .ledger
            .submission(id)
            .map_err(|_| "ledger_unavailable")?;
        if row.as_ref().is_some_and(|r| r.principal != principal) {
            return Err("scope_denied");
        }
        Ok(CodexResultV1::Receipt {
            receipt: row.as_ref().map(receipt_from_row).transpose()?,
        })
    }

    // Called under the same mutation boundary as binding/settings changes.
    pub(super) async fn submit(
        &self,
        principal: &str,
        command: CodexCommandV1,
    ) -> ControlResult<CodexResultV1> {
        let digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&command).map_err(|_| "invalid_request")?)
        );
        let (intent_id, kind) = match &command {
            CodexCommandV1::SubmitThreadText { intent_id, .. }
            | CodexCommandV1::QueueText { intent_id, .. } => (intent_id.clone(), "queue"),
            CodexCommandV1::Steer { intent_id, .. } => (intent_id.clone(), "steer"),
            CodexCommandV1::Interrupt { intent_id, .. } => (intent_id.clone(), "interrupt"),
            _ => return Err("invalid_request"),
        };
        // Existing receipts remain queryable after disable/rebind. The same ID
        // cannot be repurposed, and an unknown result never triggers another RPC.
        let received = if let Some(row) = self
            .ledger
            .submission(&intent_id)
            .map_err(|_| "ledger_unavailable")?
        {
            if row.principal != principal || row.input_digest != digest {
                return Err("intent_conflict");
            }
            if row.status != "received" {
                return self.receipt(principal, &intent_id);
            }
            if row.expires_at_ms <= now_ms() || row.input_text.is_none() && kind != "interrupt" {
                self.ledger
                    .transition_submission(
                        &intent_id,
                        "received",
                        SubmissionTransition {
                            status: "rejected".into(),
                            queue_id: None,
                            turn_id: None,
                            error_code: Some("expired".into()),
                            at_ms: now_ms(),
                        },
                    )
                    .map_err(|_| "ledger_unavailable")?;
                return self.receipt(principal, &intent_id);
            }
            true
        } else {
            false
        };
        let direct = matches!(command, CodexCommandV1::SubmitThreadText { .. });
        let (binding_id, revision, text, expected_turn, endpoint, thread) =
            if let CodexCommandV1::SubmitThreadText {
                endpoint_id,
                thread_id,
                text,
                ..
            } = command
            {
                // Validate the configured route before persisting anything. No observation
                // subscription, history reconciliation or notification setup is needed.
                self.endpoint(&endpoint_id).await?;
                let row = self.sender_binding(&endpoint_id, &thread_id).await?;
                (
                    row.id,
                    row.revision as u64,
                    Some(text),
                    None,
                    endpoint_id,
                    thread_id,
                )
            } else {
                let (binding_id, revision, text, expected_turn) = match command {
                    CodexCommandV1::QueueText {
                        binding_id,
                        expected_revision,
                        text,
                        ..
                    } => (binding_id, expected_revision, Some(text), None),
                    CodexCommandV1::Steer {
                        binding_id,
                        expected_revision,
                        text,
                        expected_turn_id,
                        ..
                    } => (
                        binding_id,
                        expected_revision,
                        Some(text),
                        Some(expected_turn_id),
                    ),
                    CodexCommandV1::Interrupt {
                        binding_id,
                        expected_revision,
                        turn_id,
                        ..
                    } => (binding_id, expected_revision, None, Some(turn_id)),
                    _ => return Err("invalid_request"),
                };
                let state = self.current_binding(&binding_id, revision).await?;
                let allowed = match kind {
                    "queue" => state.can_queue,
                    "steer" => state.can_steer,
                    _ => state.can_interrupt,
                };
                if !allowed {
                    return Err("control_unavailable");
                }
                if expected_turn.is_some() && expected_turn != state.turn_id {
                    return Err("turn_mismatch");
                }
                (
                    binding_id,
                    revision,
                    text,
                    expected_turn,
                    state.binding.endpoint_id,
                    state.binding.thread_id,
                )
            };
        let client = self.endpoint(&endpoint).await?;
        let now = now_ms();
        let row = CodexSubmissionRow {
            intent_id: intent_id.clone(),
            principal: principal.into(),
            binding_id: Some(binding_id),
            binding_revision: Some(revision.try_into().map_err(|_| "invalid_revision")?),
            operation_kind: kind.into(),
            expected_turn_id: expected_turn.clone(),
            source_kind: "local".into(),
            bot_id: None,
            chat_id: None,
            message_id: None,
            update_id: None,
            client_message_id: (kind != "interrupt").then(|| intent_id.clone()),
            status: "received".into(),
            queue_id: None,
            turn_id: None,
            input_text: text.clone(),
            input_digest: digest,
            created_at_ms: now,
            updated_at_ms: now,
            expires_at_ms: now + 15 * 60 * 1000,
            error_code: None,
        };
        if !received
            && !self
                .ledger
                .insert_submission(row)
                .map_err(|_| "ledger_unavailable")?
        {
            return self.receipt(principal, &intent_id);
        }
        if !self
            .ledger
            .transition_submission(
                &intent_id,
                "received",
                SubmissionTransition {
                    status: "submitting".into(),
                    queue_id: None,
                    turn_id: None,
                    error_code: None,
                    at_ms: now,
                },
            )
            .map_err(|_| "ledger_unavailable")?
        {
            return self.receipt(principal, &intent_id);
        }
        let input = text
            .map(|text| json!([{"type":"text","text":text,"text_elements":[]}]))
            .unwrap_or(Value::Null);
        let (method, params) = match kind {
            "queue" => (
                "thread/queue/add",
                json!({"threadId":thread,"input":input,"clientUserMessageId":intent_id}),
            ),
            "steer" => (
                "turn/steer",
                json!({"threadId":thread,"input":input,"clientUserMessageId":intent_id,"expectedTurnId":expected_turn}),
            ),
            _ => (
                "turn/interrupt",
                json!({"threadId":thread,"turnId":expected_turn}),
            ),
        };
        let preparation = if direct {
            match super::codex_control::loaded(&client).await {
                Ok(threads) if threads.contains(&thread) => Ok(()),
                Ok(_) => Self::resume_original_thread(&client, &thread)
                    .await
                    .map(|_| ()),
                Err(code) => Err(code),
            }
        } else {
            Ok(())
        };
        if let Err(code) = preparation {
            self.ledger
                .transition_submission(
                    &intent_id,
                    "submitting",
                    SubmissionTransition {
                        status: if code == "codex_result_unknown" {
                            "unknown"
                        } else {
                            "rejected"
                        }
                        .into(),
                        queue_id: None,
                        turn_id: None,
                        error_code: Some(code.into()),
                        at_ms: now_ms(),
                    },
                )
                .map_err(|_| "ledger_unavailable")?;
            return self.receipt(principal, &intent_id);
        }
        let result = client.request(method, params).await;
        let (status, queue_id, turn_id, error_code) = match result {
            Ok(value) if kind == "queue" => match value
                .pointer("/queuedSubmission/id")
                .and_then(Value::as_str)
            {
                Some(id) => ("accepted", Some(id.into()), None, None),
                None => ("unknown", None, None, Some("invalid_codex_response")),
            },
            Ok(value) if kind == "steer" => match value["turnId"]
                .as_str()
                .filter(|id| Some(*id) == expected_turn.as_deref())
            {
                Some(id) => ("accepted", None, Some(id.into()), None),
                None => ("unknown", None, None, Some("invalid_codex_response")),
            },
            Ok(_) => ("accepted", None, expected_turn, None),
            Err(RpcFailure::Unknown) => ("unknown", None, None, Some("codex_result_unknown")),
            Err(error) => ("rejected", None, None, Some(error.code())),
        };
        self.ledger
            .transition_submission(
                &intent_id,
                "submitting",
                SubmissionTransition {
                    status: status.into(),
                    queue_id,
                    turn_id,
                    error_code: error_code.map(str::to_owned),
                    at_ms: now_ms(),
                },
            )
            .map_err(|_| "ledger_unavailable")?;
        self.receipt(principal, &intent_id)
    }
}

fn receipt_from_row(row: &CodexSubmissionRow) -> ControlResult<CodexSubmissionReceiptV1> {
    Ok(CodexSubmissionReceiptV1 {
        intent_id: row.intent_id.clone(),
        status: match row.status.as_str() {
            "received" => CodexSubmissionStatusV1::Received,
            "submitting" => CodexSubmissionStatusV1::Submitting,
            "accepted" => CodexSubmissionStatusV1::Accepted,
            "rejected" => CodexSubmissionStatusV1::Rejected,
            "unknown" => CodexSubmissionStatusV1::Unknown,
            _ => return Err("invalid_receipt"),
        },
        queue_id: row.queue_id.clone(),
        turn_id: row.turn_id.clone(),
        error_code: row.error_code.clone(),
        label: match (row.status.as_str(), row.operation_kind.as_str()) {
            ("accepted", "queue") => "Accepted and queued",
            ("accepted", "steer") => "Input accepted by the original task",
            ("accepted", "interrupt") => "Interrupt requested for the original task",
            ("accepted", _) => "Accepted",
            ("received", _) => "Received; waiting to submit",
            ("submitting", _) => "Submitting to Codex",
            ("rejected", _) => "Not submitted; input retained",
            _ => "Result unknown; input retained and no automatic retry",
        }
        .into(),
        clear_input: row.status == "accepted"
            && matches!(row.operation_kind.as_str(), "queue" | "steer"),
    })
}
