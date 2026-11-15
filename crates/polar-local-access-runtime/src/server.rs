//! Restricted Polar stream endpoint with injected instance authority. Reuses the extension executor;
//! it does not initialize a Station Host, tmux, another Kernel or an SDK store.
use crate::{
    codec as local_ipc,
    local_extensions::{LocalExtensionRegistry, LocalExtensionSession},
};
use anyhow::{bail, ensure, Context, Result};
use polar_local_access_contract::{
    PolarAccessClientFrame as ClientFrame, PolarAccessServerFrame as ServerFrame,
    POLAR_ACCESS_EXTENSION, POLAR_ACCESS_STREAM_VERSION,
};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::{
    io::BufReader,
    net::UnixStream,
    sync::{mpsc, watch, Semaphore},
};

/// Authorization is supplied by the owning domain, never inferred from socket permissions.
pub trait PeerAdmission: Send + Sync {
    fn changes(&self) -> watch::Receiver<u64>;
    fn authorize(
        &self,
        uid: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<u64>> + Send + '_>>;
}

/// Owned by the executable's hosted composition; shutdown invalidates all
/// connections before the Kernel is released. There are no detached tasks.
pub struct PolarAccessSocketServer {
    node_id: String,
    uid: u32,
    admission: Option<Arc<dyn PeerAdmission>>,
    registry: Arc<LocalExtensionRegistry>,
    connections: Arc<Semaphore>,
    next_connection: AtomicU64,
    shutdown: watch::Receiver<bool>,
}
impl PolarAccessSocketServer {
    pub fn new(
        node_id: String,
        uid: u32,
        registry: LocalExtensionRegistry,
        shutdown: watch::Receiver<bool>,
    ) -> Result<Self> {
        ensure!(
            !node_id.is_empty() && node_id.len() <= 256,
            "invalid Polar identity"
        );
        ensure!(
            registry
                .offers()
                .iter()
                .all(|g| g.name == POLAR_ACCESS_EXTENSION),
            "Admin stream may expose only the Polar access profile"
        );
        Ok(Self {
            node_id,
            uid,
            admission: None,
            registry: Arc::new(registry),
            connections: Arc::new(Semaphore::new(64)),
            next_connection: AtomicU64::new(1),
            shutdown,
        })
    }

    pub fn with_admission(mut self, admission: Arc<dyn PeerAdmission>) -> Self {
        self.admission = Some(admission);
        self
    }

    pub async fn serve(&self, mut reader: BufReader<UnixStream>) -> Result<()> {
        let _slot = self
            .connections
            .clone()
            .try_acquire_owned()
            .context("Polar stream connection limit")?;
        let peer_uid = reader.get_ref().peer_cred()?.uid();
        // Subscribe before the read to avoid missing a revoke during admission.
        let mut changes = self.admission.as_ref().map(|authority| authority.changes());
        let admitted_revision = if let Some(authority) = &self.admission {
            authority.authorize(peer_uid).await?
        } else {
            ensure!(
                peer_uid == self.uid,
                "Polar stream requires the instance owner's OS identity"
            );
            0
        };
        let hello = tokio::time::timeout(
            Duration::from_secs(5),
            local_ipc::read_json_frame::<_, ClientFrame>(&mut reader),
        )
        .await??;
        let Some(ClientFrame::Hello {
            version,
            expected_node_id,
            extensions,
        }) = hello
        else {
            bail!("Polar stream requires a versioned hello");
        };
        ensure!(
            version == POLAR_ACCESS_STREAM_VERSION,
            "unsupported Polar stream version"
        );
        ensure!(
            expected_node_id == self.node_id,
            "Polar stream identity mismatch"
        );
        let grants = self.registry.negotiate(&extensions)?;
        ensure!(
            grants.iter().any(|g| g.name == POLAR_ACCESS_EXTENSION),
            "Polar access capability not negotiated"
        );
        let ready = ServerFrame::Ready {
            version,
            node_id: self.node_id.clone(),
            extensions: grants.clone(),
        };
        tokio::time::timeout(
            Duration::from_secs(5),
            local_ipc::write_json_frame(reader.get_mut(), &ready),
        )
        .await??;
        let sequence = self
            .next_connection
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
            .map_err(|_| anyhow::anyhow!("connection generation exhausted"))?;
        let (mut reader, mut writer) = tokio::io::split(reader);
        let (events, mut event_rx) = mpsc::channel(32);
        let mut session = LocalExtensionSession::new(
            self.registry.clone(),
            grants,
            format!("polar-access-{sequence}"),
            format!("polar-access-{sequence}"),
            Some(peer_uid),
        )
        .with_event_sender(events);
        let lifetime = session.lifetime();
        let mut shutdown = self.shutdown.clone();
        let mut reading = Box::pin(local_ipc::read_json_frame::<_, ClientFrame>(&mut reader));
        loop {
            if *shutdown.borrow() {
                break;
            }
            let reply = tokio::select! {
                biased;
                changed = shutdown.changed() => { let _ = changed; break; },
                _ = lifetime.cancelled() => break,
                changed = async {
                    match &mut changes {
                        Some(changes) => changes.changed().await,
                        None => std::future::pending().await,
                    }
                } => {
                    if changed.is_err() { break; }
                    if let Some(authority) = &self.admission {
                        if authority.authorize(peer_uid).await.ok() != Some(admitted_revision) { break; }
                    }
                    continue;
                },
                frame = &mut reading => {
                    // Preserve partial frame state across unrelated completions.
                    drop(reading);
                    reading = Box::pin(local_ipc::read_json_frame::<_, ClientFrame>(&mut reader));
                    match frame? {
                        None => break,
                        Some(ClientFrame::Request {request}) => session.submit(request)?
                            .map(|response| ServerFrame::Response {response}),
                        Some(ClientFrame::Cancel {request}) => Some(ServerFrame::Response {response: session.cancel(request)?}),
                        Some(ClientFrame::Ping {nonce}) => Some(ServerFrame::Pong {nonce}),
                        Some(ClientFrame::Hello {..}) => bail!("duplicate Polar stream hello"),
                    }
                },
                completion = session.next_response(), if !session.is_empty() => {
                    completion?.map(|response| ServerFrame::Response {response})
                },
                event = event_rx.recv() => match event {
                    Some(event) => Some(ServerFrame::Event {event}),
                    _ => bail!("invalid Polar event lane"),
                },
            };
            if let Some(reply) = reply {
                tokio::time::timeout(
                    Duration::from_secs(2),
                    local_ipc::write_json_frame(&mut writer, &reply),
                )
                .await??;
            }
        }
        // Drop aborts all request futures and revokes the feed authority.
        drop(session);
        Ok(())
    }
}


