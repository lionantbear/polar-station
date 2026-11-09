//! Station's adapter to the existing upstream remote client; no embedded startup.
use codex_app_server_client::{
    AppServerClient, AppServerEvent, AppServerRequestHandle, RemoteAppServerClient,
    RemoteAppServerConnectArgs, RemoteAppServerEndpoint,
};
use codex_app_server_protocol::{ClientRequest, ServerNotification, ServerRequest, ThreadItem};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

pub(super) struct EndpointEvent {
    pub endpoint_id: String,
    pub generation: u64,
    pub event: AppServerEvent,
}

pub(super) struct CodexEndpoint {
    handle: AppServerRequestHandle,
    local_socket_path: String,
    next_request: AtomicU64,
    connected: Arc<AtomicBool>,
    stop: Option<oneshot::Sender<()>>,
    forward: Option<Arc<super::ssh_socket_forward::SshSocketForward>>,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum RpcFailure {
    InvalidRequest,
    Unavailable,
    Rejected,
    Unknown,
}

impl RpcFailure {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_codex_request",
            Self::Unavailable => "codex_unavailable",
            Self::Rejected => "codex_rejected",
            Self::Unknown => "codex_result_unknown",
        }
    }
}

impl CodexEndpoint {
    pub async fn connect(
        endpoint_id: String,
        generation: u64,
        socket_path: &str,
        events: mpsc::Sender<EndpointEvent>,
        forward: Option<Arc<super::ssh_socket_forward::SshSocketForward>>,
    ) -> Result<Self, RpcFailure> {
        let endpoint = RemoteAppServerEndpoint::UnixSocket {
            socket_path: socket_path
                .try_into()
                .map_err(|_| RpcFailure::InvalidRequest)?,
        };
        let remote = RemoteAppServerClient::connect(RemoteAppServerConnectArgs {
            endpoint,
            client_name: "polar_station".into(),
            client_version: env!("CARGO_PKG_VERSION").into(),
            experimental_api: true,
            mcp_server_openai_form_elicitation: false,
            opt_out_notification_methods: vec![
                "item/agentMessage/delta".into(),
                "item/commandExecution/outputDelta".into(),
                "item/reasoning/textDelta".into(),
                "item/reasoning/summaryTextDelta".into(),
            ],
            channel_capacity: 64,
        })
        .await
        .map_err(|_| RpcFailure::Unavailable)?;
        let mut client = AppServerClient::Remote(remote);
        let handle = client.request_handle();
        let connected = Arc::new(AtomicBool::new(true));
        let live = connected.clone();
        let (stop, mut cancelled) = oneshot::channel();
        let transport = forward.clone();
        tokio::spawn(async move {
            let mut overflow = false;
            loop {
                let event = tokio::select! {
                    _ = &mut cancelled => break,
                    event = client.next_event() => event,
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
                // A full domain lane disconnects observation rather than dropping a
                // completion silently or blocking the upstream response worker.
                let send = events.try_send(EndpointEvent {
                    endpoint_id: endpoint_id.clone(),
                    generation,
                    event,
                });
                if send.is_err() || disconnected {
                    overflow = matches!(send, Err(mpsc::error::TrySendError::Full(_)));
                    break;
                }
            }
            live.store(false, Ordering::Release);
            let _ = client.shutdown().await;
            if overflow {
                // Wake failed-only recovery after the upstream client is stopped.
                // This owned forwarding task may wait for capacity; the upstream
                // RPC response worker is already shut down and never blocked.
                tokio::select! {
                    _ = &mut cancelled => {},
                    _ = events.send(EndpointEvent {
                        endpoint_id, generation,
                        event: AppServerEvent::Disconnected { message: "domain lane overflow".into() },
                    }) => {},
                }
            }
        });
        Ok(Self {
            handle,
            local_socket_path: socket_path.to_owned(),
            next_request: AtomicU64::new(1),
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

    pub async fn request(&self, method: &'static str, params: Value) -> Result<Value, RpcFailure> {
        if !self.connected() {
            return Err(RpcFailure::Unavailable);
        }
        // Decode into the pinned upstream contract, which remains the sole DTO
        // definition. Station never serializes a competing JSON-RPC protocol.
        let request: ClientRequest = serde_json::from_value(serde_json::json!({
            "id": format!("station-{}", self.next_request.fetch_add(1, Ordering::Relaxed)),
            "method": method, "params": params,
        }))
        .map_err(|_| RpcFailure::InvalidRequest)?;
        match tokio::time::timeout(Duration::from_secs(10), self.handle.request(request)).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(_))) => Err(RpcFailure::Rejected),
            Ok(Err(_)) | Err(_) => Err(RpcFailure::Unknown),
        }
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
                matches!(n.item, ThreadItem::AgentMessage { .. })
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
    }
}
