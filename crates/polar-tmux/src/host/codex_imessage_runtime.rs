//! Execution-owner outbox, independent of desktop lifetime and Telegram I/O.
use super::application_state::{wait_for_work, wait_configuration, DeliveryChannel};
use super::{
    codex_control::CodexControlService,
    imessage_sender::{now_ms, ImessageSender},
    peer_client::PeerClient,
};
use crate::host_identity::CredentialStore;
use polar_station_settings::CodexControlSettingsV1;
use polar_station_store_sqlite::{CodexNotificationRow, SqliteCodexStore};
use polar_tmux_protocol::*;
use std::{sync::Arc, time::Duration};

pub(crate) struct ImessageRuntime(tokio::task::JoinHandle<()>);
impl Drop for ImessageRuntime {
    fn drop(&mut self) {
        self.0.abort();
    }
}
struct Interrupted(SqliteCodexStore);
impl Drop for Interrupted {
    fn drop(&mut self) {
        if self.0.interrupt_imessage_io().is_err() {
            tracing::warn!("iMessage outbox interruption recovery failed");
        }
    }
}
impl ImessageRuntime {
    pub(crate) async fn shutdown(mut self) {
        self.0.abort();
        let _ = (&mut self.0).await;
    }

    pub(crate) fn start(
        service: Arc<CodexControlService>,
        local: Arc<ImessageSender>,
        credentials: Arc<dyn CredentialStore + Send + Sync>,
    ) -> Self {
        Self(tokio::spawn(async move {
            let mut changes = service.publisher.subscribe_work();
            loop {
                let config = service.config.read().await.clone();
                if !config.enabled || !config.imessage_enabled {
                    if wait_configuration(&mut changes).await.is_err_and(|e| {
                        matches!(e, tokio::sync::broadcast::error::RecvError::Closed)
                    }) {
                        break;
                    }
                    continue;
                }
                let _interrupted = Interrupted(service.ledger.clone());
                let result = tokio::select! {
                    result = run(&service,&config,&local,credentials.clone()) => result,
                    _ = async {
                        loop {
                            match wait_configuration(&mut changes).await {
                                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_))
                                    if service.config.read().await.revision != config.revision => { break; }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                                _ => {}
                            }
                        }
                    } => Ok(()),
                };
                drop(_interrupted);
                if result.is_err() {
                    tokio::select! { _=wait_for_work(&mut changes, DeliveryChannel::Imessage)=>{}, _=tokio::time::sleep(Duration::from_secs(3))=>{} }
                }
            }
        }))
    }
}

async fn run(
    service: &CodexControlService,
    config: &CodexControlSettingsV1,
    local: &ImessageSender,
    credentials: Arc<dyn CredentialStore + Send + Sync>,
) -> Result<(), &'static str> {
    let destination = config
        .imessage_sender
        .as_ref()
        .ok_or("sender_unavailable")?;
    let mut events = service.publisher.subscribe_work();
    let mut peer: Option<PeerClient> = None;
    let mut retention = tokio::time::interval(Duration::from_secs(5));
    retention.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let rows = service
            .ledger
            .pending_imessage_notifications(now_ms())
            .map_err(|_| "ledger_unavailable")?;
        let pending_receipts = !rows.is_empty();
        for row in rows {
            if serde_json::to_string(destination).ok().as_ref() != Some(&row.destination_key) {
                finish(
                    service,
                    &row,
                    &row.status,
                    if row.status == "pending" {
                        "failed"
                    } else {
                        "unknown"
                    },
                    Some("destination_changed".into()),
                )?;
                continue;
            }
            if let Some(reference) = &destination.connection_ref {
                if peer.as_ref().is_none_or(|p| !p.connected()) {
                    let connection = config
                        .host_connections
                        .iter()
                        .find(|c| &c.id == reference && c.host_id == destination.host_id)
                        .ok_or("peer_unavailable")?;
                    peer = Some(
                        PeerClient::connect(
                            service.network.as_ref().ok_or("peer_unavailable")?,
                            connection,
                            &service.owner,
                            HostPeerScopeV1::NotificationSender,
                            credentials.clone(),
                            service.publisher.clone(),
                        )
                        .await?,
                    );
                }
            } else if destination.host_id != service.owner {
                return Err("peer_unavailable");
            }

            let command = {
                let _gate = service.operations.lock().await;
                if service.config.read().await.revision != config.revision {
                    return Ok(());
                }
                if row.expires_at_ms <= now_ms() {
                    continue;
                }
                if row.status == "pending" {
                    let current = match service.binding_row(&row.binding_id) {
                        Ok(b) => b.revision == row.binding_revision && b.imessage_enabled,
                        Err("binding_not_found") => false,
                        Err(code) => return Err(code),
                    };
                    if !current {
                        finish(
                            service,
                            &row,
                            "pending",
                            "failed",
                            Some("binding_conflict".into()),
                        )?;
                        continue;
                    }
                    let text = row
                        .bounded_body
                        .clone()
                        .ok_or("notification_body_missing")?;
                    if !service
                        .ledger
                        .transition_notification(&row.id, "pending", "sending", None, None)
                        .map_err(|_| "ledger_unavailable")?
                    {
                        continue;
                    }
                    NotificationCommandV1::SendNotification {
                        request_id: row.id.clone(),
                        expected_sender_generation: destination.sender_generation.clone(),
                        destination_ref: destination.destination_ref.clone(),
                        text,
                    }
                } else {
                    NotificationCommandV1::GetReceipt {
                        request_id: row.id.clone(),
                        sender_generation: destination.sender_generation.clone(),
                    }
                }
            };
            let request = NotificationRequestEnvelopeV1 {
                protocol_version: PROTOCOL_VERSION,
                api: NotificationApiV1::NotificationV1,
                request_id: crate::host_identity::random_uuid_v4(),
                command,
            };
            let result = if let Some(peer) = &peer {
                match peer
                    .request(ClientApplicationMessage::NotificationV1(Box::new(request)))
                    .await
                {
                    Ok(HostApplicationMessage::NotificationV1(response)) => Ok(response.result),
                    _ => Err("peer_result_unknown"),
                }
            } else {
                tokio::time::timeout(
                    Duration::from_secs(20),
                    local.dispatch(&service.owner, false, request),
                )
                .await
                .map(|r| r.result)
                .map_err(|_| "sender_result_unknown")
            };
            apply_result(service, &row, result)?;
        }
        // Completed facts wake delivery immediately. Receipt retry/retention is
        // bounded independently of Codex events; no terminal polling is involved.
        loop {
            tokio::select! {
                result=wait_for_work(&mut events, DeliveryChannel::Imessage)=>{
                    if matches!(result, Err(tokio::sync::broadcast::error::RecvError::Closed)) { return Ok(()); }
                    break;
                },
                _=retention.tick()=>{
                    // Preserve the previous five-second receipt/expiry cadence.
                    // Maintenance alone does not rescan an empty delivery queue.
                    service.ledger.expire_transient_text(now_ms()).map_err(|_|"ledger_unavailable")?;
                    if pending_receipts { break; }
                },
            }
        }
    }
}

fn apply_result(
    service: &CodexControlService,
    row: &CodexNotificationRow,
    result: Result<NotificationResultV1, &'static str>,
) -> Result<(), &'static str> {
    let expected = if row.status == "pending" {
        "sending"
    } else {
        "awaiting_receipt"
    };
    let (status, error) = match result {
        Ok(NotificationResultV1::Receipt { receipt })
            if receipt.request_id == row.id
                && Some(&receipt.sender_generation) == row.sender_generation.as_ref() =>
        {
            match receipt.status {
                NotificationReceiptStatusV1::SentToMessages => ("sent", None),
                NotificationReceiptStatusV1::Failed => ("failed", receipt.error_code),
                NotificationReceiptStatusV1::Unknown => ("unknown", receipt.error_code),
                NotificationReceiptStatusV1::Sending => ("awaiting_receipt", None),
            }
        }
        Ok(NotificationResultV1::Error { code }) => {
            if row.status == "pending" && code == "sender_busy" {
                ("pending", Some(code))
            } else if row.status == "pending"
                && matches!(
                    code.as_str(),
                    "sender_disabled"
                        | "generation_mismatch"
                        | "destination_denied"
                        | "source_denied"
                        | "scope_denied"
                        | "platform_unsupported"
                        | "invalid_request"
                        | "recipient_invalid"
                        | "credential_missing"
                        | "credential_unavailable"
                        | "settings_changed"
                        | "settings_unavailable"
                        | "request_id_conflict"
                )
            {
                ("failed", Some(code))
            } else if code == "receipt_not_found" || row.status == "pending" {
                ("awaiting_receipt", Some(code))
            } else {
                ("unknown", Some(code))
            }
        }
        Err(code) => ("awaiting_receipt", Some(code.into())),
        _ => ("awaiting_receipt", Some("invalid_sender_response".into())),
    };
    finish(service, row, expected, status, error)
}
fn finish(
    service: &CodexControlService,
    row: &CodexNotificationRow,
    expected: &str,
    status: &str,
    error: Option<String>,
) -> Result<(), &'static str> {
    if service
        .ledger
        .transition_notification(&row.id, expected, status, None, error)
        .map_err(|_| "ledger_unavailable")?
        && expected != status
        && status != "pending"
    {
        service.publisher.publish(ApplicationStateApiV1::CodexV1);
    }
    Ok(())
}
