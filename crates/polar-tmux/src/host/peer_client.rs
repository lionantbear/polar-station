//! Outbound adapter for the existing scoped, encrypted Host WebSocket protocol.
use super::application_state::ApplicationStatePublisher;
use super::imessage_sender::now_ms;
use super::ssh_socket_forward::{CodexNetwork, SshSocketForward};
use crate::host_identity::CredentialStore;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures_util::{SinkExt, StreamExt};
use polar_station_settings::HostConnectionSettingsV1;
use polar_tmux_protocol::*;
use rand_core::{OsRng, RngCore};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, oneshot},
};
use tokio_tungstenite::{
    client_async_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
    WebSocketStream,
};
use zeroize::Zeroizing;

type PeerResult<T> = Result<T, &'static str>;
struct Pending {
    request: ClientApplicationMessage,
    reply: oneshot::Sender<PeerResult<HostApplicationMessage>>,
}

pub(crate) struct PeerClient {
    scope: HostPeerScopeV1,
    host_id: String,
    requests: mpsc::Sender<Pending>,
    connected: Arc<AtomicBool>,
    worker: tokio::task::JoinHandle<()>,
    forward: Option<Arc<SshSocketForward>>,
}

impl PeerClient {
    pub(crate) async fn connect(
        network: &CodexNetwork,
        connection: &HostConnectionSettingsV1,
        source: &str,
        scope: HostPeerScopeV1,
        credentials: Arc<dyn CredentialStore + Send + Sync>,
        events: ApplicationStatePublisher,
    ) -> PeerResult<Self> {
        tokio::time::timeout(Duration::from_secs(10), async {
            let reference = match scope {
                HostPeerScopeV1::ExecutionController => &connection.execution_credential_ref,
                HostPeerScopeV1::NotificationSender => &connection.notification_credential_ref,
            }
            .as_deref()
            .ok_or("peer_credential_missing")?;
            let account = format!(
                "outbound:{}",
                scope
                    .credential_account(reference)
                    .map_err(|_| "invalid_peer_reference")?
            );
            let key = tokio::task::spawn_blocking(move || {
                let encoded = Zeroizing::new(
                    credentials
                        .load("com.polarealm.PolarStation.peers", &account)
                        .map_err(|_| "peer_credential_unavailable")?
                        .ok_or("peer_credential_missing")?,
                );
                let bytes = Zeroizing::new(
                    STANDARD
                        .decode(encoded.as_bytes())
                        .map_err(|_| "invalid_peer_credential")?,
                );
                let mut key = Zeroizing::new([0; 32]);
                if bytes.len() != 32 {
                    return Err("invalid_peer_credential");
                }
                key.copy_from_slice(&bytes);
                Ok(key)
            })
            .await
            .map_err(|_| "peer_credential_unavailable")??;
            let forward = Arc::new(
                SshSocketForward::connect_host(network, &connection.ssh_target_id, connection.port)
                    .await?,
            );
            let stream = tokio::net::UnixStream::connect(forward.socket_path())
                .await
                .map_err(|_| "peer_unavailable")?;
            Self::connect_stream(
                stream,
                source,
                &connection.host_id,
                scope,
                key,
                events,
                Some(forward),
            )
            .await
        })
        .await
        .map_err(|_| "peer_connect_timeout")?
    }

    pub(crate) async fn connect_stream<S>(
        stream: S,
        source: &str,
        host: &str,
        scope: HostPeerScopeV1,
        key: Zeroizing<[u8; 32]>,
        events: ApplicationStatePublisher,
        forward: Option<Arc<SshSocketForward>>,
    ) -> PeerResult<Self>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (mut socket, writer, reader) = tokio::time::timeout(Duration::from_secs(10), async {
            let config = WebSocketConfig {
                max_message_size: Some(MAX_APPLICATION_PAYLOAD_BYTES + 128),
                max_frame_size: Some(MAX_APPLICATION_PAYLOAD_BYTES + 128),
                ..Default::default()
            };
            let (mut socket, _) =
                client_async_with_config("ws://127.0.0.1/v1/ws", stream, Some(config))
                    .await
                    .map_err(|_| "peer_unavailable")?;
            let (writer, reader) = authenticate(&mut socket, source, host, scope, &key).await?;
            Ok::<_, &'static str>((socket, writer, reader))
        })
        .await
        .map_err(|_| "peer_connect_timeout")??;
        let (requests, mut incoming) = mpsc::channel::<Pending>(8);
        let connected = Arc::new(AtomicBool::new(true));
        let live = connected.clone();
        let transport = forward.clone();
        let worker = tokio::spawn(async move {
            let mut writer = writer;
            let mut reader = reader;
            loop {
                if transport.as_ref().is_some_and(|f| !f.current()) {
                    break;
                }
                tokio::select! {
                    pending = incoming.recv() => {
                        let Some(pending) = pending else { break };
                        if pending.reply.is_closed() { continue; }
                        let result = tokio::time::timeout(Duration::from_secs(20), exchange(&mut socket, &mut writer, &mut reader, &pending.request, scope, &events, transport.as_deref())).await.unwrap_or(Err("peer_result_unknown"));
                        let failed = result.is_err();
                        let _ = pending.reply.send(result);
                        if failed { break; }
                    }
                    message = socket.next() => {
                        if transport.as_ref().is_some_and(|f| !f.current()) { break; }
                        match message {
                            Some(Ok(Message::Binary(frame))) => {
                                let Ok(bytes) = reader.open(&frame) else { break };
                                if !state_event(&bytes, scope, &events) { break; }
                            }
                            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {},
                            _ => break,
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(30)) => {
                        if !matches!(tokio::time::timeout(Duration::from_secs(5), socket.send(Message::Ping(Vec::new()))).await, Ok(Ok(()))) { break; }
                    }
                }
            }
            live.store(false, Ordering::Release);
            if scope == HostPeerScopeV1::NotificationSender {
                events.work(super::application_state::DeliveryChannel::Connector);
                events.work(super::application_state::DeliveryChannel::Imessage);
            }
            events.publish(match scope {
                HostPeerScopeV1::ExecutionController => ApplicationStateApiV1::CodexV1,
                HostPeerScopeV1::NotificationSender => ApplicationStateApiV1::NotificationV1,
            });
            // Dropping the stream and its owned forward cannot affect the remote Host.
        });
        Ok(Self {
            scope,
            host_id: host.into(),
            requests,
            connected,
            worker,
            forward,
        })
    }

    pub(crate) fn connected(&self) -> bool {
        self.connected.load(Ordering::Acquire) && self.forward.as_ref().is_none_or(|f| f.current())
    }

    pub(crate) async fn request(
        &self,
        request: ClientApplicationMessage,
    ) -> PeerResult<HostApplicationMessage> {
        request.validate().map_err(|_| "invalid_peer_request")?;
        match (&request, self.scope) {
            (ClientApplicationMessage::CodexV1(r), HostPeerScopeV1::ExecutionController)
                if r.owner_host_id == self.host_id => {}
            (ClientApplicationMessage::NotificationV1(_), HostPeerScopeV1::NotificationSender) => {}
            _ => return Err("peer_scope_denied"),
        }
        if !self.connected() {
            return Err("peer_unavailable");
        }
        let (reply, response) = oneshot::channel();
        self.requests
            .try_send(Pending { request, reply })
            .map_err(|_| "peer_unavailable")?;
        match tokio::time::timeout(Duration::from_secs(20), response).await {
            Ok(Ok(result)) => result,
            _ => {
                self.connected.store(false, Ordering::Release);
                self.worker.abort();
                Err("peer_result_unknown")
            }
        }
    }
}
impl Drop for PeerClient {
    fn drop(&mut self) {
        self.worker.abort();
    }
}

async fn authenticate<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    source: &str,
    host: &str,
    scope: HostPeerScopeV1,
    key: &[u8; 32],
) -> PeerResult<(EncryptedFrameWriter, EncryptedFrameReader)> {
    let mut nonce = [0; 32];
    OsRng.fill_bytes(&mut nonce);
    let nonce = STANDARD.encode(nonce);
    let hello = PairingClientMessage::HostPeerHello {
        protocol_version: PROTOCOL_VERSION,
        host_id: host.into(),
        source_host_id: source.into(),
        scope,
        client_nonce: nonce.clone(),
    };
    hello.validate().map_err(|_| "invalid_peer_identity")?;
    socket
        .send(Message::Text(
            serde_json::to_string(&hello).map_err(|_| "invalid_peer_identity")?,
        ))
        .await
        .map_err(|_| "peer_unavailable")?;
    let PairingHostMessage::HostChallenge {
        challenge_id,
        host_id,
        server_nonce,
        expires_unix_seconds,
        host_proof,
    } = pairing_message(socket).await?
    else {
        return Err("peer_authentication_failed");
    };
    let now = now_ms() / 1000;
    if host_id != host || expires_unix_seconds < now || expires_unix_seconds > now + 120 {
        return Err("peer_identity_mismatch");
    }
    let transcript = PairingTranscript {
        protocol_version: PROTOCOL_VERSION,
        host_id,
        device_id: format!(
            "peer:{}",
            scope
                .credential_account(source)
                .map_err(|_| "invalid_peer_identity")?
        ),
        challenge_id: challenge_id.clone(),
        client_nonce_base64: nonce,
        server_nonce_base64: server_nonce,
    };
    verify(
        key,
        &transcript,
        PairingProofDirection::HostChallenge,
        &host_proof,
    )?;
    let proof =
        production_pairing_proof_base64(key, &transcript, PairingProofDirection::ClientProof)
            .map_err(|_| "peer_authentication_failed")?;
    socket
        .send(Message::Text(
            serde_json::to_string(&PairingClientMessage::ClientProof {
                challenge_id,
                proof,
            })
            .map_err(|_| "peer_authentication_failed")?,
        ))
        .await
        .map_err(|_| "peer_unavailable")?;
    let PairingHostMessage::HostAccepted { host_proof, .. } = pairing_message(socket).await? else {
        return Err("peer_authentication_failed");
    };
    if now_ms() / 1000 > expires_unix_seconds {
        return Err("peer_authentication_failed");
    }
    verify(
        key,
        &transcript,
        PairingProofDirection::HostAccepted,
        &host_proof,
    )?;
    let keys = derive_production_session_keys(key, &transcript)
        .map_err(|_| "peer_authentication_failed")?;
    Ok((
        EncryptedFrameWriter::new(*keys.client_to_host(), FrameDirection::ClientToHost),
        EncryptedFrameReader::new(*keys.host_to_client(), FrameDirection::HostToClient),
    ))
}
fn verify(
    key: &[u8; 32],
    transcript: &PairingTranscript,
    direction: PairingProofDirection,
    proof: &str,
) -> PeerResult<()> {
    let bytes = STANDARD
        .decode(proof)
        .map_err(|_| "peer_authentication_failed")?;
    verify_production_pairing_proof(key, transcript, direction, &bytes)
        .map_err(|_| "peer_authentication_failed")
}
async fn pairing_message<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
) -> PeerResult<PairingHostMessage> {
    let Some(Ok(Message::Text(text))) = socket.next().await else {
        return Err("peer_authentication_failed");
    };
    if text.len() > 4096 {
        return Err("peer_authentication_failed");
    }
    let message: PairingHostMessage =
        serde_json::from_str(&text).map_err(|_| "peer_authentication_failed")?;
    message
        .validate()
        .map_err(|_| "peer_authentication_failed")?;
    Ok(message)
}
fn state_event(bytes: &[u8], scope: HostPeerScopeV1, events: &ApplicationStatePublisher) -> bool {
    let Ok(LocalHostFrame::ApplicationStateChanged { api, .. }) =
        serde_json::from_slice::<LocalHostFrame>(bytes)
    else {
        return false;
    };
    let expected = match scope {
        HostPeerScopeV1::ExecutionController => ApplicationStateApiV1::CodexV1,
        HostPeerScopeV1::NotificationSender => ApplicationStateApiV1::NotificationV1,
    };
    if api != expected {
        return false;
    }
    if api == ApplicationStateApiV1::NotificationV1 {
        events.work(super::application_state::DeliveryChannel::Connector);
        events.work(super::application_state::DeliveryChannel::Imessage);
    }
    events.publish(api);
    true
}
async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    writer: &mut EncryptedFrameWriter,
    reader: &mut EncryptedFrameReader,
    request: &ClientApplicationMessage,
    scope: HostPeerScopeV1,
    events: &ApplicationStatePublisher,
    forward: Option<&SshSocketForward>,
) -> PeerResult<HostApplicationMessage> {
    let payload = serde_json::to_vec(request).map_err(|_| "invalid_peer_request")?;
    let frame = writer.seal(&payload).map_err(|_| "invalid_peer_request")?;
    socket
        .send(Message::Binary(frame))
        .await
        .map_err(|_| "peer_result_unknown")?;
    loop {
        if forward.is_some_and(|f| !f.current()) {
            return Err("peer_result_unknown");
        }
        let bytes = match socket.next().await {
            Some(Ok(Message::Binary(frame))) => {
                reader.open(&frame).map_err(|_| "peer_result_unknown")?
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            _ => return Err("peer_result_unknown"),
        };
        if forward.is_some_and(|f| !f.current()) {
            return Err("peer_result_unknown");
        }
        if state_event(&bytes, scope, events) {
            continue;
        }
        let response: HostApplicationMessage =
            serde_json::from_slice(&bytes).map_err(|_| "peer_result_unknown")?;
        response.validate().map_err(|_| "peer_result_unknown")?;
        let matched = match (request, &response) {
            (
                ClientApplicationMessage::NotificationV1(a),
                HostApplicationMessage::NotificationV1(b),
            ) => a.request_id == b.request_id,
            (ClientApplicationMessage::CodexV1(a), HostApplicationMessage::CodexV1(b)) => {
                a.request_id == b.request_id && a.owner_host_id == b.owner_host_id
            }
            _ => false,
        };
        if !matched {
            return Err("peer_result_unknown");
        }
        return Ok(response);
    }
}
