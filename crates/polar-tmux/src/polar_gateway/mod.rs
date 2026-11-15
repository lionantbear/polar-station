//! A route adapter over existing local-owner capabilities, not an SDK dispatcher.
use crate::polar_target_config as config;
mod link;
mod ssh;
mod session_view;
use std::{collections::BTreeMap, sync::{Arc, Weak}};
use anyhow::Result;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use polar_tmux_protocol::{LocalExtensionOfferV1, ProtocolError, POLAR_ACCESS_EXTENSION};
use crate::{local_extensions::{LocalExtensionContext, LocalExtensionFuture,
    LocalExtensionHandler, LocalExtensionLifetime, LocalExtensionRegistry},
    target_registry::TargetRegistry, ssh_connection_manager::SshConnectionManager};
use config::{PolarTargetConfig, PolarRoute};
use link::RemoteLink;

type AccessResult<T> = std::result::Result<T, ProtocolError>;
fn failure(code: &str, text: &str) -> ProtocolError { ProtocolError::new(code, text, false) }
fn offer() -> LocalExtensionOfferV1 {
    LocalExtensionOfferV1 { name: POLAR_ACCESS_EXTENSION.into(), version: 1, max_payload_bytes: 64 * 1024 }
}
struct LinkSlot { route: PolarTargetConfig, owner: Weak<LocalExtensionLifetime>, link: Mutex<Option<Arc<RemoteLink>>> }
type LinkKey = (usize, String);
pub(crate) struct PolarGateway {
    forwards: ssh::ForwardOwner,
    embedded: Option<Arc<dyn LocalExtensionHandler>>,
    links: Mutex<BTreeMap<LinkKey, Arc<LinkSlot>>>,
    targets: TargetRegistry,
    ssh: SshConnectionManager,
}

pub(crate) fn install(registry: &mut LocalExtensionRegistry, targets: &TargetRegistry,
                      ssh: &SshConnectionManager) -> Result<Option<Arc<PolarGateway>>> {
    let routes = targets.polar_targets();
    PolarTargetConfig::validate_all(&routes)?;
    let embedded = registry.take_handler(POLAR_ACCESS_EXTENSION);
    let gateway = Arc::new(PolarGateway {embedded, links: Mutex::new(BTreeMap::new()),
        targets: targets.clone(), ssh: ssh.clone(), forwards: ssh::ForwardOwner::default()});
    registry.register(gateway.clone())?;
    Ok(Some(gateway))
}
impl PolarGateway {
    /// Reuse the same bounded protocol, lifetime and Vault authorization as Chat.
    pub(crate) async fn check(self: &Arc<Self>, id: &str) -> AccessResult<()> {
        use crate::local_extensions::LocalExtensionSession;
        use polar_tmux_protocol::{LocalExtensionRequestV1, LocalExtensionResultV1};
        let mut registry = LocalExtensionRegistry::default();
        registry.register(self.clone()).map_err(|_| failure("check_unavailable", "Polar check could not start."))?;
        let (events, _receiver) = tokio::sync::mpsc::channel(2);
        let mut session = LocalExtensionSession::new(Arc::new(registry), vec![offer()],
            "connection-check".into(), crate::host_identity::random_uuid_v4(), Some(crate::local_ipc::effective_uid()))
            .with_event_sender(events);
        let immediate = session.submit(LocalExtensionRequestV1 { extension: POLAR_ACCESS_EXTENSION.into(), version: 1,
            request_id: crate::host_identity::random_uuid_v4(), timeout_ms: 10_000, payload: json!({"target_id":id,"op":"describe"}) })
            .map_err(|_| failure("check_unavailable", "Polar check could not start."))?;
        let response = match immediate {
            Some(response) => response,
            None => session.next_response().await.ok().flatten().ok_or_else(|| failure("check_unavailable", "Polar check did not return a result."))?,
        };
        match response.result {
            LocalExtensionResultV1::Success { .. } => Ok(()),
            LocalExtensionResultV1::Rejected { error } => Err(error),
            _ => Err(failure("check_unavailable", "Polar check is unavailable.")),
        }
    }

    pub(crate) async fn references(&self, id: &str, machine: bool) -> Vec<String> {
        self.links.lock().await.values().filter(|slot| slot.owner.upgrade().is_some_and(|owner| owner.is_live()) &&
            if machine { matches!(&slot.route.route, PolarRoute::SshUds { execution_target_id, .. } if execution_target_id == id) } else { slot.route.id == id })
            .map(|slot| format!("Chat: {}", slot.route.display_name)).collect()
    }

    pub(crate) async fn shutdown(&self) {
        self.links.lock().await.clear();
        self.forwards.shutdown().await;
    }

    async fn route(&self, c: &LocalExtensionContext, route: &PolarTargetConfig) -> AccessResult<Arc<RemoteLink>> {
        let slot = {
            let mut links = self.links.lock().await;
            links.retain(|(_, id), slot| slot.owner.upgrade().is_some_and(|owner| owner.is_live()) &&
                (id != &route.id || (slot.route.route == route.route && slot.route.expected_node_id == route.expected_node_id)));
            let key = (Arc::as_ptr(&c.lifetime) as usize, route.id.clone());
            if !links.contains_key(&key) && links.len() >= 64 {
                return Err(failure("target_capacity", "Close unused target connections before opening more."));
            }
            links.entry(key).or_insert_with(|| Arc::new(LinkSlot {
                route: route.clone(), owner: Arc::downgrade(&c.lifetime), link: Mutex::new(None)
            })).clone()
        };
        // The global map lock is never held across network I/O. Concurrent
        // requests share one initializer only for their own target/connection.
        let mut link = slot.link.lock().await;
        if let Some(current) = link.as_ref().filter(|v| v.is_live()) {return Ok(current.clone());}
        *link = None;
        let connecting = async {
            match &route.route {
                PolarRoute::LocalUds {socket_path, expected_peer_uid} => RemoteLink::connect_local(socket_path,
                    expected_peer_uid.unwrap_or_else(crate::local_ipc::effective_uid),
                    &route.expected_node_id, &route.id, c).await
                    .map_err(|_| failure("polar_socket_or_identity_failed", "Cannot verify the Polar socket, peer UID or pinned Node ID. Check the instance path, permissions and identity.")),
                PolarRoute::SshUds {execution_target_id, remote_socket_path} => {
                    let forward = self.forwards.connect(&self.ssh, &self.targets,
                        execution_target_id, remote_socket_path, c).await
                        .map_err(|_| failure("ssh_forward_failed", "Cannot connect SSH or forward the configured Polar socket. Check the SSH destination, authenticated account and remote socket path."))?;
                    let stream = tokio::net::UnixStream::connect(&forward.socket_path).await
                        .map_err(|_| failure("polar_socket_unavailable", "The forwarded Polar socket is unavailable. Check the running instance and socket path."))?;
                    RemoteLink::from_stream(stream, &route.expected_node_id, &route.id, c, Some(forward), crate::local_ipc::effective_uid()).await
                        .map_err(|_| failure("polar_identity_failed", "The Polar stream identity or protocol could not be verified. Check the pinned Node ID and running instance."))
                },
            }
        };
        let connected = tokio::select! {
            biased;
            _ = c.lifetime.cancelled() => return Err(failure("target_disconnected", "The desktop connection ended.")),
            result = tokio::time::timeout_at(c.deadline, connecting) => result,
        };
        let connected = connected.map_err(|_| failure("target_connect_timeout", "Polar connection timed out. Check the SSH route and running instance."))??;
        let connected = Arc::new(connected); *link = Some(connected.clone()); Ok(connected)
    }
    async fn dispatch(&self, c: LocalExtensionContext, mut payload: Value) -> AccessResult<Value> {
        if !c.lifetime.is_live() || c.peer_uid != Some(crate::local_ipc::effective_uid()) || c.deadline <= tokio::time::Instant::now() {
            return Err(failure("target_access_denied", "A current same-user desktop connection is required."));
        }
        let object = payload.as_object_mut().ok_or_else(|| failure("invalid_polar_request", "An object request is required."))?;
        let target = match object.remove("target_id") {
            None => "embedded".to_owned(), // compatibility with the embedded-only client
            Some(Value::String(id)) if !id.is_empty() && id.len() <= 128 => id,
            _ => return Err(failure("invalid_polar_target", "An explicit target must be a nonempty target ID.")),
        };
        let op = object.get("op").and_then(Value::as_str).unwrap_or("").to_owned();
        if op == "list_targets" {
            if object.len() != 1 {return Err(failure("invalid_polar_request", "Target enumeration takes no extra parameters."));}
            let mut items = Vec::new();
            if self.embedded.is_some() {items.push(json!({"target_id":"embedded","display_name":"Embedded Polar","status":"available","origin":"embedded"}));}
            items.extend(self.targets.polar_targets().iter().map(|route| json!({"target_id":route.id,"display_name":route.display_name,
                "expected_node_id":route.expected_node_id,"status":"configured","origin":match &route.route {PolarRoute::LocalUds{..}=>"standalone",PolarRoute::SshUds{..}=>"ssh"}})));
            return Ok(json!({"targets":items}));
        }
        if !matches!(op.as_str(), "describe"|"channels"|"threads"|"bind"|"renew"|"read"|"activity"|"send"|"inquiry"|"release"|"subscribe"|"subscribe_catalog"|"unsubscribe") {
            return Err(failure("unsupported_polar_operation", "This operation is not in the Station access allowlist."));
        }
        if target == "embedded" {
            let port = self.embedded.as_ref().ok_or_else(|| failure("unknown_target", "No embedded Polar is configured."))?;
            let prepare=op=="inquiry" && payload["action"]["kind"]=="prepare_view";
            let activate=payload["action"]["activate"].as_bool()==Some(true);
            let result=port.request(c, payload).await?;
            return if prepare {self.prepare_terminal_view(None,result,activate).await} else {Ok(result)};
        }
        let routes = self.targets.polar_targets();
        let route = routes.iter().find(|r| r.id == target)
            .ok_or_else(|| failure("unknown_target", "Select an explicitly configured Polar target."))?;
        if op != "describe" && payload["expected_node_id"].as_str() != Some(route.expected_node_id.as_str()) {
            return Err(failure("target_identity_changed", "The request identity differs from the configured target pin."));
        }
        // A restored Thread lease may not establish a new connection. It is
        // only valid on the connection that issued it; the remote handler
        // rechecks this on every operation, including after awaits.
        let link = self.route(&c, route).await?;
        let activate_view = payload["action"]["activate"].as_bool() == Some(true);
        let payload_for_view = op == "inquiry" && payload["action"]["kind"] == "prepare_view";
        let result = link.request(&c, payload).await?;
        if op == "describe" && result["node_id"].as_str() != Some(route.expected_node_id.as_str()) {
            return Err(failure("target_identity_changed", "The described Polar identity differs from the configured pin."));
        }
        if op == "inquiry" && payload_for_view {
            return self.prepare_terminal_view(Some(route), result, activate_view).await;
        }
        Ok(result)
    }
}
impl LocalExtensionHandler for PolarGateway {
    fn offer(&self) -> LocalExtensionOfferV1 {offer()}
    fn request<'a>(&'a self, context: LocalExtensionContext, payload: Value) -> LocalExtensionFuture<'a> {
        Box::pin(self.dispatch(context, payload))
    }
}


