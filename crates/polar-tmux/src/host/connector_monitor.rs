//! Source outbox and reply handoff over the existing scoped Host peer transport.
use super::application_state::{wait_for_work, DeliveryChannel};
use super::{codex_control::CodexControlService, imessage_sender::now_ms, peer_client::PeerClient};
use polar_station_settings::{ConnectorProviderKind, MessageConnectorV1};
use polar_tmux_protocol::*;
use std::{collections::BTreeMap, sync::Arc, time::Duration};

pub(crate) struct ConnectorMonitor(tokio::task::JoinHandle<()>);
impl Drop for ConnectorMonitor {
    fn drop(&mut self) {
        self.0.abort();
    }
}
impl ConnectorMonitor {
    pub(crate) async fn shutdown(mut self) {
        self.0.abort();
        let _ = (&mut self.0).await;
    }

    pub(crate) fn start(service: Arc<CodexControlService>) -> Self {
        Self(tokio::spawn(async move {
            let mut changes = service.publisher.subscribe_work();
            let mut peers = BTreeMap::new();
            let mut retention = tokio::time::interval(Duration::from_secs(60));
            retention.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                let failed = run(&service, &mut peers).await.is_err();
                loop {
                    tokio::select! {
                        result=wait_for_work(&mut changes, DeliveryChannel::Connector)=>{
                            if matches!(result, Err(tokio::sync::broadcast::error::RecvError::Closed)) { return; }
                            break;
                        },
                        _=tokio::time::sleep(Duration::from_secs(3)), if failed=>break,
                        _=retention.tick()=>{
                            // Retention is local maintenance, never a remote status poll.
                            if service.ledger.expire_connector_text(now_ms()).is_err() {
                                tracing::warn!("Connector retention failed");
                            }
                        },
                    }
                }
            }
        }))
    }
}
async fn call(
    service: &CodexControlService,
    peers: &mut BTreeMap<
        String,
        (
            polar_station_settings::HostConnectionSettingsV1,
            Arc<PeerClient>,
        ),
    >,
    connector: &MessageConnectorV1,
    command: ConnectorCommandV1,
) -> Result<ConnectorResultV1, &'static str> {
    if connector.connection_ref.is_none() {
        if connector.host_id != service.owner {
            return Err("connector_host_mismatch");
        }
        return Ok(service
            .connector_service()
            .command(&service.owner, true, command)
            .unwrap_or_else(|code| ConnectorResultV1::Error { code: code.into() }));
    }
    let config = service.config.read().await.clone();
    let connection = config
        .host_connections
        .iter()
        .find(|c| {
            Some(&c.id) == connector.connection_ref.as_ref() && c.host_id == connector.host_id
        })
        .ok_or("peer_unavailable")?;
    if peers
        .get(&connector.id)
        .is_none_or(|(c, p)| c != connection || !p.connected())
    {
        let peer = PeerClient::connect(
            service.network.as_ref().ok_or("peer_unavailable")?,
            connection,
            &service.owner,
            HostPeerScopeV1::NotificationSender,
            service.peer_credentials.clone(),
            service.publisher.clone(),
        )
        .await?;
        peers.insert(connector.id.clone(), (connection.clone(), Arc::new(peer)));
    }
    let peer = &peers.get(&connector.id).ok_or("peer_unavailable")?.1;
    match peer
        .request(ClientApplicationMessage::NotificationV1(Box::new(
            NotificationRequestEnvelopeV1 {
                protocol_version: PROTOCOL_VERSION,
                api: NotificationApiV1::NotificationV1,
                request_id: crate::host_identity::random_uuid_v4(),
                command: NotificationCommandV1::Connector {
                    command: Box::new(command),
                },
            },
        )))
        .await?
    {
        HostApplicationMessage::NotificationV1(response) => match response.result {
            NotificationResultV1::Connector { result } => Ok(*result),
            _ => Err("invalid_connector_response"),
        },
        _ => Err("invalid_connector_response"),
    }
}
async fn run(
    service: &CodexControlService,
    peers: &mut BTreeMap<
        String,
        (
            polar_station_settings::HostConnectionSettingsV1,
            Arc<PeerClient>,
        ),
    >,
) -> Result<(), &'static str> {
    let settings = service
        .settings
        .load_message_connectors()
        .map_err(|_| "settings_unavailable")?;
    peers.retain(|id, _| settings.connectors.iter().any(|c| c.enabled && &c.id == id));
    let mut retry = false;
    for connector in settings.connectors.iter().filter(|c| c.enabled) {
        let subscribed = call(
            service,
            peers,
            connector,
            ConnectorCommandV1::Subscribe {
                connector_id: connector.id.clone(),
            },
        )
        .await;
        match subscribed {
            Ok(ConnectorResultV1::Inbound { messages }) => {
                for message in messages {
                    if let Some(status) = deliver_inbound(service, &message).await? {
                        match call(
                            service,
                            peers,
                            connector,
                            ConnectorCommandV1::AcknowledgeInbound {
                                connector_id: connector.id.clone(),
                                inbound_id: message.id,
                                status: status.into(),
                            },
                        )
                        .await
                        {
                            Ok(ConnectorResultV1::Acknowledged) => {
                                // Subscribe is deliberately one-item bounded. An
                                // acknowledgement exposes the next durable item.
                                service.publisher.work(DeliveryChannel::Connector);
                                service
                                    .publisher
                                    .publish(ApplicationStateApiV1::NotificationV1);
                            }
                            _ => retry = true,
                        }
                    }
                }
            }
            _ => retry = true,
        }
    }
    for row in service
        .ledger
        .pending_connector_notifications(now_ms())
        .map_err(|_| "ledger_unavailable")?
    {
        let p: ConnectorPublicationV1 =
            serde_json::from_str(row.bounded_body.as_deref().ok_or("notification_expired")?)
                .map_err(|_| "invalid_connector_publication")?;
        let Some(connector) = settings.connectors.iter().find(|c| {
            c.enabled && c.id == p.connector_id && Some(&c.host_id) == row.sender_host_id.as_ref()
        }) else {
            service
                .ledger
                .transition_notification(
                    &row.id,
                    &row.status,
                    if row.status == "pending" {
                        "failed"
                    } else {
                        "unknown"
                    },
                    None,
                    Some("connector_changed".into()),
                )
                .map_err(|_| "ledger_unavailable")?;
            continue;
        };
        let command = if row.status == "pending" {
            ConnectorCommandV1::Publish {
                publication: p.clone(),
            }
        } else {
            ConnectorCommandV1::GetReceipt {
                connector_id: connector.id.clone(),
                delivery_id: p.delivery_id(),
            }
        };
        if row.status == "pending" {
            service
                .ledger
                .transition_notification(&row.id, "pending", "awaiting_receipt", None, None)
                .map_err(|_| "ledger_unavailable")?;
        }
        let mut result = call(service, peers, connector, command).await;
        if matches!(&result,Ok(ConnectorResultV1::Error{code}) if code=="receipt_not_found") {
            result = call(
                service,
                peers,
                connector,
                ConnectorCommandV1::Publish {
                    publication: p.clone(),
                },
            )
            .await;
        }
        match result {
            Ok(ConnectorResultV1::Receipt {
                delivery_id,
                stream_id,
                status,
                provider_message_id,
                error_code,
            }) if delivery_id == p.delivery_id() && stream_id == p.stream_id() => {
                if matches!(status.as_str(), "sent" | "failed" | "unknown" | "expired") {
                    service
                        .ledger
                        .transition_notification(
                            &row.id,
                            "awaiting_receipt",
                            &status,
                            provider_message_id.and_then(|id| id.parse().ok()),
                            error_code,
                        )
                        .map_err(|_| "ledger_unavailable")?;
                    service.publisher.publish(ApplicationStateApiV1::CodexV1);
                }
            }
            Ok(ConnectorResultV1::Error { code }) => {
                service
                    .ledger
                    .transition_notification(
                        &row.id,
                        "awaiting_receipt",
                        "failed",
                        None,
                        Some(code),
                    )
                    .map_err(|_| "ledger_unavailable")?;
                service.publisher.publish(ApplicationStateApiV1::CodexV1);
            }
            _ => retry = true,
        }
    }
    if retry {
        Err("connector_unavailable")
    } else {
        Ok(())
    }
}
async fn deliver_inbound(
    service: &CodexControlService,
    message: &ConnectorInboundV1,
) -> Result<Option<&'static str>, &'static str> {
    if message.expires_at_ms <= now_ms() {
        return Ok(Some("rejected"));
    }
    let _gate = service.operations.lock().await;
    let principal = format!("connector:{}", message.connector_id);
    // A persisted submission is authoritative even if the binding revision changed.
    if let Some(row) = service
        .ledger
        .submission(&message.id)
        .map_err(|_| "ledger_unavailable")?
    {
        if row.principal != principal {
            return Ok(Some("rejected"));
        }
        return Ok(Some(match row.status.as_str() {
            "accepted" => "accepted",
            "rejected" => "rejected",
            _ => "unknown",
        }));
    }
    let Some(binding) = service
        .ledger
        .bindings()
        .map_err(|_| "ledger_unavailable")?
        .into_iter()
        .find(|b| {
            b.enabled
                && b.owner_host_id == service.owner
                && b.endpoint_id == message.endpoint_id
                && b.thread_id == message.thread_id
        })
    else {
        return Ok(None);
    };
    let result = service
        .submit(
            &principal,
            CodexCommandV1::QueueText {
                binding_id: binding.id,
                expected_revision: binding
                    .revision
                    .try_into()
                    .map_err(|_| "invalid_revision")?,
                intent_id: message.id.clone(),
                text: message.text.clone(),
            },
        )
        .await;
    match result {
        Ok(CodexResultV1::Receipt {
            receipt: Some(receipt),
        }) => Ok(Some(match receipt.status {
            CodexSubmissionStatusV1::Accepted => "accepted",
            CodexSubmissionStatusV1::Rejected => "rejected",
            _ => "unknown",
        })),
        Err(
            "control_disabled"
            | "operation_not_allowed"
            | "control_unavailable"
            | "codex_disconnected"
            | "binding_not_found",
        ) => Ok(None),
        Err(_) => Ok(Some("rejected")),
        _ => Ok(Some("unknown")),
    }
}

pub(super) fn connector_channel(provider: ConnectorProviderKind) -> &'static str {
    if provider == ConnectorProviderKind::Telegram {
        "telegram"
    } else {
        "imessage"
    }
}
