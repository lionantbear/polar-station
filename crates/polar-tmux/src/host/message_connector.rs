//! Shared provider stream authority. Observers submit original event identities.
use super::{application_state::ApplicationStatePublisher, imessage_sender::now_ms};
use polar_station_settings::{ConnectorProviderKind, MessageConnectorV1, SettingsRepository};
use polar_station_store_sqlite::{ConnectorDeliveryRow, ConnectorStreamRow, SqliteCodexStore};
use polar_tmux_protocol::*;
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub(crate) struct ConnectorService {
    pub(super) host_id: String,
    pub(super) settings: SettingsRepository,
    pub(super) ledger: SqliteCodexStore,
    pub(super) publisher: ApplicationStatePublisher,
}
type Result<T> = std::result::Result<T, &'static str>;
impl ConnectorService {
    pub(crate) fn new(
        host_id: String,
        settings: SettingsRepository,
        ledger: SqliteCodexStore,
        publisher: ApplicationStatePublisher,
    ) -> Self {
        Self {
            host_id,
            settings,
            ledger,
            publisher,
        }
    }
    pub(crate) fn dispatch(
        &self,
        principal: &str,
        local: bool,
        request: NotificationRequestEnvelopeV1,
    ) -> NotificationResponseEnvelopeV1 {
        let result = if request.validate().is_err() {
            Err("invalid_request")
        } else {
            match request.command {
                NotificationCommandV1::Connector { command } => {
                    self.command(principal, local, *command)
                }
                _ => Err("unsupported_command"),
            }
        };
        NotificationResponseEnvelopeV1 {
            protocol_version: PROTOCOL_VERSION,
            api: request.api,
            request_id: request.request_id,
            result: NotificationResultV1::Connector {
                result: Box::new(
                    result.unwrap_or_else(|code| ConnectorResultV1::Error { code: code.into() }),
                ),
            },
        }
    }
    pub(super) fn authorize(&self, id: &str, source: &str) -> Result<MessageConnectorV1> {
        self.settings
            .load_message_connectors()
            .map_err(|_| "settings_unavailable")?
            .connectors
            .into_iter()
            .find(|c| {
                c.id == id
                    && c.enabled
                    && c.host_id == self.host_id
                    && c.connection_ref.is_none()
                    && (source == self.host_id
                        || c.allowed_source_hosts.iter().any(|h| h == source))
            })
            .ok_or("connector_denied")
    }
    pub(super) fn command(
        &self,
        principal: &str,
        local: bool,
        command: ConnectorCommandV1,
    ) -> Result<ConnectorResultV1> {
        command.validate().map_err(|_| "invalid_request")?;
        let source = if local {
            self.host_id.as_str()
        } else {
            principal
        };
        match command {
            ConnectorCommandV1::GetStreams { connector_id } => {
                self.authorize(&connector_id, source)?;
                let streams = self
                    .ledger
                    .connector_streams(&connector_id)
                    .map_err(|_| "ledger_unavailable")?
                    .into_iter()
                    .filter(|s| local || s.return_host_id == source)
                    .map(|s| ConnectorStreamV1 {
                        id: s.id,
                        connector_id: s.connector_id,
                        destination_ref: s.destination_ref,
                        thread_id: s.thread_id,
                        title: s.title,
                        return_host_id: s.return_host_id,
                        return_endpoint_id: s.return_endpoint_id,
                        status: s.status,
                        platform_id: s.platform_id,
                        error_code: s.error_code,
                    })
                    .collect();
                Ok(ConnectorResultV1::Streams { streams })
            }
            ConnectorCommandV1::SetReturnRoute {
                stream_id,
                expected_host_id,
                host_id,
                endpoint_id,
            } => {
                if !local {
                    return Err("scope_denied");
                }
                let stream = self
                    .ledger
                    .connector_stream(&stream_id)
                    .map_err(|_| "ledger_unavailable")?
                    .ok_or("stream_not_found")?;
                self.authorize(&stream.connector_id, &host_id)?;
                if !self
                    .ledger
                    .change_connector_return_route(
                        &stream_id,
                        &expected_host_id,
                        &host_id,
                        &endpoint_id,
                    )
                    .map_err(|_| "ledger_unavailable")?
                {
                    return Err("return_route_conflict");
                }
                self.publisher
                    .publish(ApplicationStateApiV1::NotificationV1);
                self.publisher.work(super::application_state::DeliveryChannel::Connector);
                Ok(ConnectorResultV1::Acknowledged)
            }
            ConnectorCommandV1::GetSettings => {
                if !local {
                    return Err("scope_denied");
                }
                Ok(ConnectorResultV1::Settings {
                    host_id: self.host_id.clone(),
                    settings: self
                        .settings
                        .load_message_connectors()
                        .map_err(|_| "settings_unavailable")?,
                })
            }
            ConnectorCommandV1::UpdateSettings {
                expected_revision,
                settings,
            } => {
                if !local {
                    return Err("scope_denied");
                }
                // One configured Telegram account is served by the existing single bot consumer.
                if settings
                    .connectors
                    .iter()
                    .filter(|c| {
                        c.enabled
                            && c.host_id == self.host_id
                            && c.connection_ref.is_none()
                            && c.provider == ConnectorProviderKind::Telegram
                    })
                    .count()
                    > 1
                {
                    return Err("duplicate_telegram_connector");
                }
                for c in &settings.connectors {
                    if (c.host_id == self.host_id) != c.connection_ref.is_none() {
                        return Err("connector_host_mismatch");
                    }
                }
                let settings = self
                    .settings
                    .update_message_connectors(expected_revision, settings)
                    .map_err(|e| match e {
                        polar_station_settings::UpdateError::RevisionConflict { .. } => {
                            "revision_conflict"
                        }
                        _ => "settings_unavailable",
                    })?;
                self.publisher
                    .publish(ApplicationStateApiV1::NotificationV1);
                self.publisher.configuration(super::application_state::ConfigurationKind::Connectors, settings.revision);
                Ok(ConnectorResultV1::Settings {
                    host_id: self.host_id.clone(),
                    settings,
                })
            }
            ConnectorCommandV1::Publish { publication: p } => {
                let connector = self.authorize(&p.connector_id, source)?;
                let provider_scope = match connector.provider {
                    ConnectorProviderKind::Telegram => {
                        let config = self
                            .settings
                            .load_codex_control()
                            .map_err(|_| "settings_unavailable")?;
                        let destination = config
                            .telegram
                            .destinations
                            .iter()
                            .find(|d| d.id == p.destination_ref)
                            .filter(|_| config.telegram.enabled)
                            .ok_or("destination_unavailable")?;
                        format!("{}:{}", destination.bot_id, destination.chat_id)
                    }
                    // Ordinary AppleScript send is not a native threaded reply.
                    ConnectorProviderKind::Imessage => return Err("native_thread_unavailable"),
                    _ => return Err("provider_unsupported"),
                };
                let now = now_ms();
                let id = p.delivery_id();
                let stream_id = p.stream_id();
                let stream = ConnectorStreamRow {
                    id: stream_id.clone(),
                    connector_id: p.connector_id,
                    destination_ref: p.destination_ref,
                    thread_id: p.thread_id,
                    title: p.thread_name,
                    provider_scope,
                    return_host_id: source.into(),
                    return_endpoint_id: p.endpoint_id,
                    status: "pending".into(),
                    platform_id: None,
                    error_code: None,
                    created_at_ms: now,
                };
                let row = ConnectorDeliveryRow {
                    id: id.clone(),
                    stream_id,
                    turn_id: p.turn_id,
                    event_kind: p.event_kind,
                    content_digest: format!("{:x}", Sha256::digest(p.text.as_bytes())),
                    body: Some(p.text),
                    status: "pending".into(),
                    provider_message_id: None,
                    error_code: None,
                    created_at_ms: now,
                    updated_at_ms: now,
                    expires_at_ms: now.saturating_add(86400000),
                };
                if !self
                    .ledger
                    .admit_connector_delivery(stream, row, source.into())
                    .map_err(|_| "ledger_unavailable")?
                {
                    return Err("delivery_content_conflict");
                }
                self.publisher
                    .publish(ApplicationStateApiV1::NotificationV1);
                self.publisher.work(super::application_state::DeliveryChannel::Telegram);
                self.receipt(&id, source)
            }
            ConnectorCommandV1::GetReceipt {
                connector_id,
                delivery_id,
            } => {
                self.authorize(&connector_id, source)?;
                let row = self
                    .ledger
                    .connector_delivery(&delivery_id, Some(source))
                    .map_err(|_| "ledger_unavailable")?
                    .ok_or("receipt_not_found")?;
                let stream = self
                    .ledger
                    .connector_stream(&row.stream_id)
                    .map_err(|_| "ledger_unavailable")?
                    .ok_or("stream_not_found")?;
                if stream.connector_id != connector_id {
                    return Err("receipt_not_found");
                }
                self.receipt(&delivery_id, source)
            }
            ConnectorCommandV1::Subscribe { connector_id } => {
                self.authorize(&connector_id, source)?;
                let rows = self
                    .ledger
                    .connector_inbound(&connector_id, source, now_ms())
                    .map_err(|_| "ledger_unavailable")?;
                let mut messages = Vec::new();
                for row in rows {
                    let stream = self
                        .ledger
                        .connector_stream(&row.stream_id)
                        .map_err(|_| "ledger_unavailable")?
                        .ok_or("stream_not_found")?;
                    messages.push(ConnectorInboundV1 {
                        id: row.id,
                        connector_id: connector_id.clone(),
                        stream_id: row.stream_id,
                        endpoint_id: stream.return_endpoint_id,
                        thread_id: stream.thread_id,
                        text: row.body.ok_or("inbound_expired")?,
                        expires_at_ms: row.expires_at_ms,
                    });
                }
                Ok(ConnectorResultV1::Inbound { messages })
            }
            ConnectorCommandV1::AcknowledgeInbound {
                connector_id,
                inbound_id,
                status,
            } => {
                self.authorize(&connector_id, source)?;
                self.ledger
                    .acknowledge_connector_inbound(&inbound_id, &connector_id, source, &status)
                    .map_err(|_| "ledger_unavailable")?;
                Ok(ConnectorResultV1::Acknowledged)
            }
        }
    }
    fn receipt(&self, id: &str, source: &str) -> Result<ConnectorResultV1> {
        let row = self
            .ledger
            .connector_delivery(id, Some(source))
            .map_err(|_| "ledger_unavailable")?
            .ok_or("receipt_not_found")?;
        Ok(ConnectorResultV1::Receipt {
            delivery_id: row.id,
            stream_id: row.stream_id,
            status: row.status,
            provider_message_id: row.provider_message_id,
            error_code: row.error_code,
        })
    }
}
#[cfg(feature = "codex-control")]
impl super::codex_control::CodexControlService {
    pub(super) fn connector_service(&self) -> ConnectorService {
        ConnectorService::new(
            self.owner.clone(),
            self.settings.clone(),
            self.ledger.clone(),
            self.publisher.clone(),
        )
    }
}
