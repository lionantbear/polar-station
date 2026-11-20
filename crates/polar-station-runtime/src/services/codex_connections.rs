//! Connection establishment is isolated by endpoint/peer. Registry locks never
//! cover SSH, authentication, websocket handshakes or application requests.
use super::{
    codex_control::{CodexControlService, ControlResult, EndpointConnection},
    codex_endpoint::{CodexEndpoint, RpcFailure},
    codex_remote::RemoteConnection,
    peer_client::PeerClient,
};
use polar_station_protocol::HostPeerScopeV1;
use polar_station_settings::{CodexControlModeV1, HostConnectionSettingsV1};
use std::{
    collections::BTreeMap,
    sync::{atomic::Ordering, Arc, Weak},
};
use tokio::sync::Mutex;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ConnectionKey {
    Endpoint(String),
    Peer(String),
    LocalDaemonStart,
}
#[derive(Default)]
pub(super) struct ConnectionAttempts(Mutex<BTreeMap<ConnectionKey, Weak<Mutex<()>>>>);
impl ConnectionAttempts {
    pub(super) async fn gate(&self, key: ConnectionKey) -> Arc<Mutex<()>> {
        let mut attempts = self.0.lock().await;
        attempts.retain(|_, weak| weak.strong_count() > 0);
        if let Some(gate) = attempts.get(&key).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(Mutex::new(()));
        attempts.insert(key, Arc::downgrade(&gate));
        gate
    }
}
impl CodexControlService {
    pub(super) async fn endpoint(&self, id: &str) -> ControlResult<Arc<CodexEndpoint>> {
        let gate = self
            .connection_attempts
            .gate(ConnectionKey::Endpoint(id.into()))
            .await;
        let _attempt = gate.lock().await;
        let config = self.config.read().await;
        let endpoint = config
            .endpoints
            .iter()
            .find(|e| e.id == id)
            .cloned()
            .ok_or("endpoint_not_found")?;
        drop(config);
        if endpoint.owner_host_id != self.owner {
            return Err("owner_mismatch");
        }
        if endpoint.control_mode == CodexControlModeV1::RemoteHost {
            return Err("endpoint_mode_unavailable");
        }
        let mut endpoints = self.endpoints.lock().await;
        if let Some(connection) = endpoints
            .get(id)
            .filter(|e| e.client.connected() && e.settings == endpoint)
        {
            return Ok(connection.client.clone());
        }
        endpoints.remove(id);
        drop(endpoints);
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let forward = if endpoint.control_mode == CodexControlModeV1::SshDirect {
            Some(Arc::new(
                super::ssh_socket_forward::SshSocketForward::connect(
                    self.network.as_ref().ok_or("ssh_unavailable")?,
                    &endpoint.execution_target_id,
                    &endpoint.socket_path,
                )
                .await?,
            ))
        } else {
            None
        };
        let socket = forward
            .as_ref()
            .map(|f| f.socket_path().to_string_lossy().into_owned())
            .unwrap_or_else(|| endpoint.socket_path.clone());
        let client = Arc::new(
            CodexEndpoint::connect(id.into(), generation, &socket, self.events.clone(), forward)
                .await
                .map_err(RpcFailure::code)?,
        );
        // A settings change may finish while SSH or handshake is in flight.
        // Keep this read guard through publication, never through network I/O.
        let config = self.config.read().await;
        if !config.endpoints.contains(&endpoint) {
            return Err("settings_conflict");
        }
        self.endpoints.lock().await.insert(
            id.into(),
            EndpointConnection {
                generation,
                settings: endpoint,
                client: client.clone(),
            },
        );
        drop(config);
        self.publisher.publish(polar_station_protocol::ApplicationStateApiV1::ControlCenterV1);
        Ok(client)
    }

    pub(super) async fn peer_connection(
        &self,
        connection: &HostConnectionSettingsV1,
    ) -> ControlResult<Arc<PeerClient>> {
        let gate = self
            .connection_attempts
            .gate(ConnectionKey::Peer(connection.id.clone()))
            .await;
        let _attempt = gate.lock().await;
        {
            let mut peers = self.peers.lock().await;
            if let Some(peer) = peers
                .get(&connection.id)
                .filter(|p| p.connection == *connection && p.client.connected())
            {
                return Ok(peer.client.clone());
            }
            peers.remove(&connection.id);
        }
        let client = Arc::new(
            PeerClient::connect(
                self.network.as_ref().ok_or("ssh_unavailable")?,
                connection,
                &self.owner,
                HostPeerScopeV1::ExecutionController,
                self.peer_credentials.clone(),
                self.publisher.clone(),
            )
            .await?,
        );
        let config = self.config.read().await;
        if !config.host_connections.contains(connection) {
            return Err("settings_conflict");
        }
        self.peers.lock().await.insert(
            connection.id.clone(),
            RemoteConnection {
                connection: connection.clone(),
                client: client.clone(),
            },
        );
        Ok(client)
    }
}
