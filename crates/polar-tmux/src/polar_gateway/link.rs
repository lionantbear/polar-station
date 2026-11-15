//! One bounded upstream connection per authenticated desktop/Polar target.
//! No retry of writes, no parallel SDK implementation, and no polling for data.
use std::{collections::BTreeMap, path::Path, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Duration};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use tokio::{io::AsyncWrite, net::UnixStream, sync::{mpsc, oneshot, watch, Semaphore}, task::AbortHandle};
use polar_tmux_protocol::{PolarAccessClientFrame as ClientFrame, PolarAccessServerFrame as ServerFrame,
    LocalExtensionRequestV1, LocalExtensionCancelV1, LocalExtensionResultV1, LocalExtensionOfferV1,
    ProtocolError, POLAR_ACCESS_EXTENSION, POLAR_ACCESS_STREAM_VERSION, validate_extension_ack};
use crate::{local_ipc, local_extensions::{LocalExtensionContext, LocalExtensionEvents}};
use super::{failure, AccessResult};

const MAX_PENDING: usize = 16;
type Reply = oneshot::Sender<AccessResult<Value>>;
enum Command { Request {request: LocalExtensionRequestV1, reply: Reply}, Cancel(String) }
struct Pending {
    reply: Reply,
    subscription: Option<String>,
    unsubscribing: Option<String>,
    releasing: Option<String>,
}
struct Subscription { sequence: u64, boot: Option<String>, lease: String }

pub(super) struct RemoteLink {
    sender: mpsc::Sender<Command>,
    request_slots: Arc<Semaphore>,
    stop: watch::Sender<bool>,
    live: Arc<AtomicBool>,
    abort: AbortHandle,
    route: Option<std::sync::Weak<super::ssh::RouteLease>>,
}
impl Drop for RemoteLink {
    fn drop(&mut self) { let _ = self.stop.send(true); self.abort.abort(); }
}
struct Liveness(Arc<AtomicBool>);
impl Drop for Liveness { fn drop(&mut self) { self.0.store(false, Ordering::Release); } }
struct CancelOnDrop { sender: mpsc::Sender<Command>, stop: watch::Sender<bool>, id: Option<String> }
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            // If cancellation cannot be queued, close this target only. Do not
            // leave a request running with no observer indefinitely.
            if self.sender.try_send(Command::Cancel(id)).is_err() { let _ = self.stop.send(true); }
        }
    }
}
impl RemoteLink {
    pub(super) fn is_live(&self) -> bool { self.live.load(Ordering::Acquire) && !*self.stop.borrow() &&
        self.route.as_ref().is_none_or(|route| route.upgrade().is_some_and(|r| r.is_current())) }

    pub(super) async fn connect_local(path: &Path, expected_peer_uid: u32, node: &str, target: &str,
        context: &LocalExtensionContext) -> Result<Self> {
        validate_local_socket(path, expected_peer_uid)?;
        let stream = UnixStream::connect(path).await.context("connect configured Polar UDS")?;
        ensure!(stream.peer_cred()?.uid() == expected_peer_uid, "Polar peer UID mismatch");
        // Recheck the pathname after connect, never disclose data to a replaced
        // foreign socket. The already connected peer is independently checked.
        validate_local_socket(path, expected_peer_uid)?;
        Self::from_stream(stream, node, target, context, None, expected_peer_uid).await
    }

    pub(super) async fn from_stream(mut stream: UnixStream, node: &str, target: &str,
        context: &LocalExtensionContext, route: Option<Arc<super::ssh::RouteLease>>, expected_peer_uid: u32) -> Result<Self> {
        ensure!(stream.peer_cred()?.uid() == expected_peer_uid, "Polar or SSH forwarding peer UID mismatch");
        ensure!(route.as_ref().is_none_or(|r| r.is_current()), "SSH lease changed before handshake");
        let offered = vec![super::offer()];
        let hello = ClientFrame::Hello {version: POLAR_ACCESS_STREAM_VERSION,
            expected_node_id: node.to_owned(), extensions: offered.clone()};
        tokio::time::timeout(Duration::from_secs(5), local_ipc::write_json_frame(&mut stream, &hello)).await??;
        let ready = tokio::time::timeout(Duration::from_secs(5),
            local_ipc::read_json_frame::<_, ServerFrame>(&mut stream)).await??;
        let Some(ServerFrame::Ready {version, node_id, extensions}) = ready else { bail!("Polar stream upgrade unavailable"); };
        ensure!(version == POLAR_ACCESS_STREAM_VERSION && node_id == node, "Polar identity changed");
        validate_extension_ack(&offered, &extensions)?;
        let grant = extensions.into_iter().find(|g| g.name == POLAR_ACCESS_EXTENSION)
            .context("Polar access was not negotiated")?;
        ensure!(route.as_ref().is_none_or(|r| r.is_current()), "SSH lease changed during handshake");
        let events = context.events.clone().context("desktop event lane is unavailable")?;
        let (sender, commands) = mpsc::channel(32);
        let (stop, stopping) = watch::channel(false);
        let live = Arc::new(AtomicBool::new(true));
        let guard = Liveness(live.clone());
        let context = context.clone(); let target = target.to_owned();
        let weak_route = route.as_ref().map(Arc::downgrade);
        let task = tokio::spawn(async move {
            let _guard = guard;
            let mut subscriptions = BTreeMap::new();
            let _result = run(stream, commands, stopping, route.as_deref(), &context, &events,
                &target, &grant, &mut subscriptions).await;
            // Mark unusable BEFORE emitting reset. A native read-rebind triggered
            // by the reset must not accidentally reuse this dying connection.
            _guard.0.store(false, Ordering::Release);
            // Every reset names the whole target. One successful notification
            // invalidates all its leases in the shared reducer. Bound total
            // cleanup time, not two seconds times the subscription count.
            let _ = tokio::time::timeout(Duration::from_secs(2), async {
                for (id, subscription) in subscriptions {
                    let Some(sequence) = subscription.sequence.checked_add(1) else {continue;};
                    if events.emit_isolated(&id, sequence, json!({"kind":"reset_required",
                        "reason":"target_disconnected", "target_id":target,
                        "boot_epoch":subscription.boot.unwrap_or_else(|| "connection-ended".into())})).await.is_ok() {
                        break;
                    }
                }
            }).await;
        });
        Ok(Self {sender, stop, live, abort: task.abort_handle(), route: weak_route,
            request_slots: Arc::new(Semaphore::new(MAX_PENDING))})
    }

    pub(super) async fn request(&self, context: &LocalExtensionContext, payload: Value) -> AccessResult<Value> {
        if !self.is_live() { return Err(disconnected()); }
        // Backpressure is isolated to this Polar, not the Host control queue.
        // A permit is held until response/cancellation, bounding server work.
        let _permit = tokio::select! {
            biased;
            _ = context.lifetime.cancelled() => return Err(disconnected()),
            permit = tokio::time::timeout_at(context.deadline, self.request_slots.clone().acquire_owned()) => {
                permit.ok().and_then(|result|result.ok())
                    .ok_or_else(|| failure("target_busy", "The selected Polar request window is full."))?
            }
        };
        if !self.is_live() { return Err(disconnected()); }
        let remaining = context.deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() { return Err(disconnected()); }
        let request = LocalExtensionRequestV1 {extension: POLAR_ACCESS_EXTENSION.into(), version: 1,
            request_id: context.request_id.clone(), timeout_ms: remaining.as_millis().clamp(1, 30_000) as u32, payload};
        request.validate().map_err(|_| failure("invalid_polar_request", "The Polar request is invalid."))?;
        let (reply, result) = oneshot::channel();
        self.sender.try_send(Command::Request {request, reply})
            .map_err(|_| failure("target_busy", "The selected Polar target queue is full or closed."))?;
        let mut cancel = CancelOnDrop {sender: self.sender.clone(), stop: self.stop.clone(), id: Some(context.request_id.clone())};
        let response = tokio::select! {
            biased;
            _ = context.lifetime.cancelled() => Err(disconnected()),
            result = tokio::time::timeout_at(context.deadline, result) => match result {
                Ok(Ok(response)) => { cancel.id = None; response },
                _ => Err(disconnected()),
            }
        };
        response
    }
}

fn disconnected() -> ProtocolError { failure("target_disconnected", "The target connection ended. A send may have completed; do not replay it automatically.") }
fn validate_local_socket(path: &Path, expected_peer_uid: u32) -> Result<()> {
    super::config::validate_socket_path(path)?;
    ensure!(path.canonicalize()? == path, "Polar socket path is not canonical");
    let socket = std::fs::symlink_metadata(path)?;
    let parent = path.parent().context("Polar socket parent missing")?;
    let directory = std::fs::symlink_metadata(parent)?;
    ensure!(socket.file_type().is_socket() && socket.uid() == expected_peer_uid, "Polar socket owner differs from configured server UID");
    ensure!(directory.is_dir() && directory.uid() == expected_peer_uid && directory.mode() & 0o022 == 0,
        "Polar socket directory must be writable only by its owner");
    Ok(())
}
async fn write<W: AsyncWrite + Unpin>(writer: &mut W, frame: &ClientFrame) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(2), local_ipc::write_json_frame(writer, frame)).await??;
    Ok(())
}
async fn route_cancelled(stop: &mut Option<watch::Receiver<bool>>) {
    let Some(stop) = stop else { std::future::pending::<()>().await; return; };
    loop {
        if *stop.borrow_and_update() { return; }
        if stop.changed().await.is_err() { return; }
    }
}
#[allow(clippy::too_many_arguments)]
async fn run(stream: UnixStream, mut commands: mpsc::Receiver<Command>, mut stop: watch::Receiver<bool>,
    route: Option<&super::ssh::RouteLease>, context: &LocalExtensionContext, events: &LocalExtensionEvents,
    target: &str, grant: &LocalExtensionOfferV1, subscriptions: &mut BTreeMap<String, Subscription>) -> Result<()> {
    let mut route_stop = route.map(|route| route.revoked.clone());
    let (mut reader, mut writer) = stream.into_split();
    let mut pending = BTreeMap::<String, Pending>::new();
    let mut reading = Box::pin(local_ipc::read_json_frame::<_, ServerFrame>(&mut reader));
    // Transport liveness only: not message polling. SSH has its own supervisor.
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut awaiting_pong = None; let mut nonce = 0_u64;
    loop {
        if *stop.borrow() { break; }
        ensure!(route.is_none_or(|r| r.is_current()), "SSH route generation changed");
        tokio::select! {
            biased;
            _ = context.lifetime.cancelled() => break,
            _ = stop.changed() => break,
            _ = route_cancelled(&mut route_stop) => break,
            _ = heartbeat.tick() => {
                ensure!(awaiting_pong.is_none(), "Polar target heartbeat expired");
                nonce = nonce.checked_add(1).context("heartbeat generation exhausted")?;
                write(&mut writer, &ClientFrame::Ping {nonce}).await?;
                awaiting_pong = Some(nonce);
            },
            command = commands.recv() => match command {
                None => break,
                Some(Command::Cancel(id)) => {
                    if let Some(p) = pending.remove(&id) {
                        // A subscription/release may have committed just before
                        // its ACK was cancelled. Closing only this upstream
                        // connection revokes all unobserved feed authority.
                        if p.subscription.is_some() || p.unsubscribing.is_some() || p.releasing.is_some() {
                            bail!("cancelled subscription lifecycle operation");
                        }
                        write(&mut writer, &ClientFrame::Cancel {request: LocalExtensionCancelV1 {
                            extension: POLAR_ACCESS_EXTENSION.into(), version: 1, request_id: id}}).await?;
                    }
                },
                Some(Command::Request {request, reply}) => {
                    ensure!(route.is_none_or(|r| r.is_current()), "SSH route changed before sending");
                    if reply.is_closed() {continue;}
                    if pending.len() >= MAX_PENDING {
                        let _ = reply.send(Err(failure("target_busy", "The selected Polar has too many pending requests.")));
                        continue;
                    }
                    ensure!(!pending.contains_key(&request.request_id), "duplicate upstream request");
                    request.validate_with_limit(grant.max_payload_bytes as usize)?;
                    let subscription = if matches!(request.payload["op"].as_str(), Some("subscribe" | "subscribe_catalog")) {
                        let id = request.payload["subscription_id"].as_str().context("subscription ID missing")?.to_owned();
                        ensure!(subscriptions.len() < 128 && !subscriptions.contains_key(&id), "subscription limit or duplicate");
                        let lease = if request.payload["op"] == "subscribe_catalog" { String::new() } else { request.payload["lease_id"].as_str().context("subscription lease missing")?.to_owned() };
                        subscriptions.insert(id.clone(), Subscription {sequence: 0, boot: None, lease}); Some(id)
                    } else {None};
                    let unsubscribing = if request.payload["op"] == "unsubscribe" {
                        request.payload["subscription_id"].as_str().map(str::to_owned)
                    } else {None};
                    let releasing = if request.payload["op"] == "release" {
                        request.payload["lease_id"].as_str().map(str::to_owned)
                    } else {None};
                    let id = request.request_id.clone();
                    pending.insert(id, Pending {reply, subscription, unsubscribing, releasing});
                    write(&mut writer, &ClientFrame::Request {request}).await?;
                },
            },
            frame = &mut reading => {
                drop(reading);
                reading = Box::pin(local_ipc::read_json_frame::<_, ServerFrame>(&mut reader));
                ensure!(route.is_none_or(|r| r.is_current()), "SSH route changed before delivery");
                match frame? {
                    None => break,
                    Some(ServerFrame::Ready {..}) => bail!("unexpected upstream handshake"),
                    Some(ServerFrame::Pong {nonce}) => {
                        ensure!(awaiting_pong == Some(nonce), "invalid upstream heartbeat"); awaiting_pong = None;
                    },
                    Some(ServerFrame::Response {response}) => {
                        response.validate_with_limit(grant.max_payload_bytes as usize)?;
                        ensure!(response.extension == POLAR_ACCESS_EXTENSION && response.version == 1, "upstream response scope mismatch");
                        if let Some(p) = pending.remove(&response.request_id) {
                            let success = matches!(response.result, LocalExtensionResultV1::Success {..});
                            if !success {if let Some(id) = &p.subscription {subscriptions.remove(id);}}
                            if success {
                                if let Some(id) = &p.unsubscribing {subscriptions.remove(id);}
                                if let Some(lease) = &p.releasing {subscriptions.retain(|_, sub| &sub.lease != lease);}
                            }
                            let owns_feed = p.subscription.is_some() || p.unsubscribing.is_some() || p.releasing.is_some();
                            let result = match response.result {
                                LocalExtensionResultV1::Success {mut payload} => {
                                    if let Some(object) = payload.as_object_mut() {object.insert("target_id".into(), json!(target));}
                                    Ok(payload)
                                },
                                LocalExtensionResultV1::Rejected {error} => Err(error),
                                _ => Err(failure("target_operation_refused", "The selected Polar refused or cancelled this operation.")),
                            };
                            if p.reply.send(result).is_err() && owns_feed {
                                bail!("subscription lifecycle ACK has no owner");
                            }
                        }
                    },
                    Some(ServerFrame::Event {event}) => {
                        event.validate_with_limit(grant.max_payload_bytes as usize)?;
                        ensure!(event.extension == POLAR_ACCESS_EXTENSION && event.version == 1, "upstream event scope mismatch");
                        if let Some(subscription) = subscriptions.get_mut(&event.subscription_id) {
                            ensure!(event.sequence == subscription.sequence.checked_add(1).context("event sequence exhausted")?, "upstream event sequence gap");
                            subscription.sequence = event.sequence;
                            if let Some(boot) = event.payload["boot_epoch"].as_str() {
                                ensure!(subscription.boot.as_deref().is_none_or(|old| old == boot), "upstream boot epoch changed");
                                subscription.boot = Some(boot.to_owned());
                            }
                            let mut payload = event.payload;
                            payload["target_id"] = json!(target);
                            events.emit_isolated(&event.subscription_id, event.sequence, payload.clone()).await
                                .map_err(|e| anyhow::anyhow!("{}",e.code))?;
                            if payload["kind"] == "reset_required" ||
                                (payload["kind"] == "snapshot_end" && payload["page"] == true) {
                                subscriptions.remove(&event.subscription_id);
                            }
                        }
                    },
                }
            },
        }
    }
    // Dropping pending senders marks all observers unconfirmed. No retransmit.
    Ok(())
}
