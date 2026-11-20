//! Station's adapter to the existing upstream remote client; no embedded startup.
use codex_app_server_protocol::{ServerNotification, ServerRequest, ThreadItemPayload};
use polar_inhouse_backend_codex::client::{
    connect_unix, next_event, AppServerEvent, SessionRequests,
};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

pub(super) struct EndpointEvent {
    pub endpoint_id: String,
    pub generation: u64,
    pub event: AppServerEvent,
}

pub(super) struct CodexEndpoint {
    handle: SessionRequests,
    local_socket_path: String,
    connected: Arc<AtomicBool>,
    stop: Option<oneshot::Sender<()>>,
    forward: Option<Arc<super::ssh_socket_forward::SshSocketForward>>,
}

pub(super) use polar_inhouse_backend_codex::client::RequestFailure as RpcFailure;

impl CodexEndpoint {
    pub async fn connect(
        endpoint_id: String,
        generation: u64,
        socket_path: &str,
        events: mpsc::Sender<EndpointEvent>,
        forward: Option<Arc<super::ssh_socket_forward::SshSocketForward>>,
    ) -> Result<Self, RpcFailure> {
        let mut client = connect_unix(
            std::path::Path::new(socket_path),
            "polar_station",
            vec![
                "item/agentMessage/delta".into(),
                "item/commandExecution/outputDelta".into(),
                "item/reasoning/textDelta".into(),
                "item/reasoning/summaryTextDelta".into(),
            ],
        )
        .await
        .map_err(|_| RpcFailure::Unavailable)?;
        let handle = SessionRequests::new(&client);
        let connected = Arc::new(AtomicBool::new(true));
        let live = connected.clone();
        let (stop, mut cancelled) = oneshot::channel();
        let transport = forward.clone();
        tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = &mut cancelled => break,
                    event = next_event(&mut client) => event.map(|received| received.event),
                };
                let event = if transport.as_ref().is_some_and(|f| !f.current()) {
                    None
                } else {
                    event
                };
                let disconnected = event
                    .as_ref()
                    .is_none_or(|e| matches!(e, AppServerEvent::Disconnected { .. }));
                if disconnected {
                    live.store(false, Ordering::Release);
                }
                let event = event.unwrap_or_else(|| AppServerEvent::Disconnected {
                    message: String::new(),
                });
                if !relevant_event(&event) {
                    continue;
                }
                // The upstream Remote client has an independent RPC worker and
                // event receiver. Backpressure here preserves ordered domain
                // facts without interrupting in-flight requests. Cancellation
                // must remain available while the domain consumer is busy.
                let send = tokio::select! {
                    _ = &mut cancelled => break,
                    send = events.send(EndpointEvent {
                        endpoint_id: endpoint_id.clone(),
                        generation,
                        event,
                    }) => send,
                };
                if send.is_err() || disconnected {
                    break;
                }
            }
            live.store(false, Ordering::Release);
            let _ = client.shutdown().await;
        });
        Ok(Self {
            handle,
            local_socket_path: socket_path.to_owned(),
            connected,
            stop: Some(stop),
            forward,
        })
    }

    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Acquire) && self.forward.as_ref().is_none_or(|f| f.current())
    }

    /// This is the local Host's private Unix-socket path. For SSH Direct
    /// endpoints the owned forwarding lease remains in `forward`.
    pub fn local_socket_path(&self) -> &str {
        &self.local_socket_path
    }

    #[tracing::instrument(
        target = "polar_station_operation",
        name = "codex_upstream_request",
        skip_all,
        fields(component = "Codex", method)
    )]
    pub async fn request(&self, method: &'static str, params: Value) -> Result<Value, RpcFailure> {
        if !self.connected() {
            return Err(RpcFailure::Unavailable);
        }
        self.handle
            .request(method, params, Duration::from_secs(10))
            .await
    }
}

impl Drop for CodexEndpoint {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

pub(super) fn relevant_event(event: &AppServerEvent) -> bool {
    match event {
        AppServerEvent::ServerNotification(n) => match n.as_ref() {
            ServerNotification::ThreadStarted(_)
            | ServerNotification::ThreadStatusChanged(_)
            | ServerNotification::ThreadNameUpdated(_)
            | ServerNotification::ThreadClosed(_)
            | ServerNotification::TurnStarted(_)
            | ServerNotification::TurnCompleted(_) => true,
            ServerNotification::ItemCompleted(n) => {
                matches!(n.item.payload, ThreadItemPayload::AgentMessage { .. })
            }
            _ => false,
        },
        AppServerEvent::ServerRequest(r) => matches!(
            r.as_ref(),
            ServerRequest::CommandExecutionRequestApproval { .. }
                | ServerRequest::FileChangeRequestApproval { .. }
                | ServerRequest::ToolRequestUserInput { .. }
        ),
        AppServerEvent::Disconnected { .. } | AppServerEvent::Lagged { .. } => true,
        // Cloud reply stream events do not describe the Station endpoint.
        AppServerEvent::ThreadStreamDisconnected(_)
        | AppServerEvent::ThreadStreamReconnected(_) => false,
    }
}
