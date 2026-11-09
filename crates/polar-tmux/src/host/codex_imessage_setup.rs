//! Local admission of an existing Mac notification capability over managed SSH.
use super::{
    codex_control::{CodexControlService, ControlResult},
    peer_client::PeerClient,
};
use polar_station_settings::{HostConnectionSettingsV1, ImessageDestinationV1};
use polar_tmux_protocol::*;

impl CodexControlService {
    pub(super) async fn connect_imessage_sender(
        &self,
        revision: u64,
        target: &str,
        port: u16,
        pairing: NotificationPairingV1,
    ) -> ControlResult<CodexResultV1> {
        let mut settings = self.config.read().await.clone();
        if settings.revision != revision {
            return Err("settings_conflict");
        }
        if pairing.source_host_id != self.owner {
            return Err("pairing_source_mismatch");
        }
        pairing.validate().map_err(|_| "invalid_pairing")?;
        let network = self.network.as_ref().ok_or("ssh_unavailable")?;
        if network
            .targets
            .get(target)
            .and_then(|t| t.ssh_destination().map(str::to_owned))
            .is_none()
        {
            return Err("ssh_target_unavailable");
        }
        if settings.imessage_sender.as_ref().is_some_and(|s| {
            s.host_id != pairing.sender_host_id
                || s.sender_generation != pairing.sender_generation
                || s.destination_ref != pairing.destination_ref
        }) {
            return Err("imessage_sender_already_configured");
        }
        let id = format!("imessage-{}", pairing.sender_host_id);
        let reference = format!("notification-{}", crate::host_identity::random_uuid_v4());
        let connection = HostConnectionSettingsV1 {
            id: id.clone(),
            host_id: pairing.sender_host_id.clone(),
            ssh_target_id: target.into(),
            port,
            execution_credential_ref: None,
            notification_credential_ref: Some(reference.clone()),
        };
        if let Some(existing) = settings.host_connections.iter().find(|c| c.id == id) {
            if existing.execution_credential_ref.is_some()
                || existing.host_id != connection.host_id
                || existing.ssh_target_id != connection.ssh_target_id
                || existing.port != port
            {
                return Err("host_connection_conflict");
            }
        }
        let account = format!(
            "outbound:{}",
            HostPeerScopeV1::NotificationSender
                .credential_account(&reference)
                .map_err(|_| "invalid_peer_reference")?
        );
        let credentials = self.peer_credentials.clone();
        let secret = zeroize::Zeroizing::new(pairing.notification_key.clone());
        tokio::task::spawn_blocking(move || {
            credentials.save("com.polarealm.PolarStation.peers", &account, &secret)
        })
        .await
        .map_err(|_| "credential_unavailable")?
        .map_err(|_| "credential_unavailable")?;
        let peer = PeerClient::connect(
            network,
            &connection,
            &self.owner,
            HostPeerScopeV1::NotificationSender,
            self.peer_credentials.clone(),
            self.publisher.clone(),
        )
        .await?;
        let response = peer
            .request(ClientApplicationMessage::NotificationV1(Box::new(
                NotificationRequestEnvelopeV1 {
                    protocol_version: PROTOCOL_VERSION,
                    api: NotificationApiV1::NotificationV1,
                    request_id: crate::host_identity::random_uuid_v4(),
                    command: NotificationCommandV1::GetSenderStatus,
                },
            )))
            .await?;
        match response {
            HostApplicationMessage::NotificationV1(response) => match response.result {
                NotificationResultV1::Status { status }
                    if status.host_id == pairing.sender_host_id
                        && status.sender_generation == pairing.sender_generation
                        && status.supported
                        && status.authorized
                        && status.enabled => {}
                _ => return Err("imessage_sender_not_ready"),
            },
            _ => return Err("invalid_peer_response"),
        }
        settings.host_connections.retain(|c| c.id != id);
        settings.host_connections.push(connection);
        settings.imessage_sender = Some(ImessageDestinationV1 {
            host_id: pairing.sender_host_id.clone(),
            connection_ref: Some(id),
            sender_generation: pairing.sender_generation.clone(),
            destination_ref: pairing.destination_ref.clone(),
        });
        settings.imessage_enabled = true;
        self.save_control_settings(revision, settings).await
    }
}
