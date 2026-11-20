//! App-server entry points shared by Inhouse Polar and Station.
pub use codex_app_server_client::{
    app_server_control_socket_path, verified_local_daemon_socket, AppServerClient, AppServerEvent,
    AppServerRequestHandle,
};
pub use codex_app_server_protocol::{ClientRequest, ServerNotification, ServerRequest};

use crate::codex_app_server_bootstrap::start_in_process_client;
use crate::CodexAgentConfig;
use codex_app_server_client::{
    RemoteAppServerClient, RemoteAppServerConnectArgs, RemoteAppServerEndpoint,
};
use std::path::Path;

/// Issues upstream requests with IDs scoped to this connection.
pub struct SessionRequests {
    handle: AppServerRequestHandle,
    next_id: std::sync::atomic::AtomicU64,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RequestFailure {
    InvalidRequest,
    Unavailable,
    Rejected,
    Unknown,
}

impl RequestFailure {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_codex_request",
            Self::Unavailable => "codex_unavailable",
            Self::Rejected => "codex_rejected",
            Self::Unknown => "codex_result_unknown",
        }
    }
}

impl SessionRequests {
    pub fn new(client: &AppServerClient) -> Self {
        Self {
            handle: client.request_handle(),
            next_id: std::sync::atomic::AtomicU64::new(1),
        }
    }

    pub async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: std::time::Duration,
    ) -> Result<serde_json::Value, RequestFailure> {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let request = serde_json::from_value(
            serde_json::json!({"id": id, "method": method, "params": params}),
        )
        .map_err(|_| RequestFailure::InvalidRequest)?;
        match tokio::time::timeout(timeout, self.handle.request(request)).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(_))) => Err(RequestFailure::Rejected),
            Ok(Err(_)) | Err(_) => Err(RequestFailure::Unknown),
        }
    }
}

/// A callback can be completed once, on the connection which received the event.
pub struct ReceivedEvent {
    pub event: AppServerEvent,
    pub callback: Option<Callback>,
}

pub async fn next_event(client: &mut AppServerClient) -> Option<ReceivedEvent> {
    let event = client.next_event().await?;
    let callback = match &event {
        AppServerEvent::ServerRequest(request) => Some(Callback {
            handle: client.request_handle(),
            id: request.id().clone(),
        }),
        _ => None,
    };
    Some(ReceivedEvent { event, callback })
}

pub struct Callback {
    handle: AppServerRequestHandle,
    id: codex_app_server_protocol::RequestId,
}

impl Callback {
    pub async fn resolve(self, value: serde_json::Value) -> std::io::Result<()> {
        self.handle.resolve_server_request(self.id, value).await
    }

    pub async fn reject(
        self,
        error: codex_app_server_protocol::JSONRPCErrorError,
    ) -> std::io::Result<()> {
        self.handle.reject_server_request(self.id, error).await
    }
}

pub async fn start_embedded(config: &CodexAgentConfig) -> anyhow::Result<AppServerClient> {
    let client = start_in_process_client(config).await?;
    Ok(AppServerClient::InProcess(client))
}

/// The public local control address of Polar's embedded Codex app-server.
/// The upstream listener creates and protects the one parent directory.
pub fn polar_control_socket(home: &Path) -> std::io::Result<std::path::PathBuf> {
    if !home.is_absolute() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "account home must be absolute"));
    }
    Ok(home.join(".polar-app-server/app-server-control.sock"))
}



pub async fn connect_unix(
    path: &Path,
    client_name: &str,
    opt_out_notification_methods: Vec<String>,
) -> anyhow::Result<AppServerClient> {
    let remote = RemoteAppServerClient::connect(RemoteAppServerConnectArgs {
        endpoint: RemoteAppServerEndpoint::UnixSocket {
            socket_path: path.to_path_buf().try_into()?,
        },
        required_human_approval: false,
        client_name: client_name.into(),
        client_version: env!("CARGO_PKG_VERSION").into(),
        experimental_api: true,
        mcp_server_openai_form_elicitation: false,
        opt_out_notification_methods,
        channel_capacity: 64,
    })
    .await?;
    Ok(AppServerClient::Remote(remote))
}

/// Owns only the child started for this profile and its fresh private socket directory.
pub(crate) struct PrivateServer {
    child: Option<Box<dyn process_wrap::tokio::ChildWrapper>>,
    _directory: tempfile::TempDir,
}

impl PrivateServer {
    pub(crate) async fn shutdown(mut self) {
        if let Some(mut child) = self.child.take() {
            if matches!(child.try_wait(), Ok(None)) {
                let _ = Box::into_pin(child.kill()).await;
            }
        }
    }
}

impl Drop for PrivateServer {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.start_kill();
            }
        }
    }
}

pub(crate) async fn start_private(
    cfg: &CodexAgentConfig,
    paths: &crate::CodexProfilePaths,
) -> anyhow::Result<(AppServerClient, PrivateServer)> {
    use crate::codex_app_server_bootstrap::managed_profile_overrides;
    use std::process::Stdio;
    use std::time::Duration;
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("server.sock");
    let mut command = tokio::process::Command::new("codex");
    command
        .env("CODEX_HOME", &paths.home)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    if let Some(cwd) = cfg.cwd.as_ref() {
        command.current_dir(cwd);
    }
    for (key, value) in managed_profile_overrides(cfg)? {
        command.arg("-c").arg(format!("{key}={value}"));
    }
    command
        .args(["app-server", "--listen"])
        .arg(format!("unix://{}", socket.display()));
    let mut command = process_wrap::tokio::CommandWrap::from(command);
    command.wrap(process_wrap::tokio::KillOnDrop);
    #[cfg(unix)]
    command.wrap(process_wrap::tokio::ProcessGroup::leader());
    #[cfg(windows)]
    command.wrap(process_wrap::tokio::JobObject);
    let mut server = PrivateServer {
        child: Some(command.spawn()?),
        _directory: directory,
    };
    let child = server.child.as_mut().expect("fresh child");
    let client = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(status) = child.try_wait()? {
                anyhow::bail!("private Codex app-server exited during startup ({status})");
            }
            if socket.exists() {
                return connect_unix(&socket, "polar-inhouse-agent", Vec::new()).await;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    Ok((client, server))
}

/// Runs one isolated read-only turn. A dropped caller owns only this embedded session.
pub async fn run_read_only_turn(
    model: Option<&str>,
    service: &str,
    instructions: &str,
    prompt: String,
    output_schema: serde_json::Value,
) -> anyhow::Result<String> {
    let cfg = CodexAgentConfig::managed(crate::CodexMode::Agentic, Default::default());
    let client = start_embedded(&cfg).await?;
    read_only_turn(client, model, service, instructions, prompt, output_schema).await
}

pub(crate) async fn read_only_turn(
    mut client: AppServerClient,
    model: Option<&str>,
    service: &str,
    instructions: &str,
    prompt: String,
    output_schema: serde_json::Value,
) -> anyhow::Result<String> {
    use codex_app_server_protocol::{
        AskForApproval, ClientRequest, JSONRPCErrorError, RequestId, SandboxMode,
        ThreadItemPayload, ThreadStartParams, ThreadStartResponse, TurnStartResponse, TurnStatus,
    };
    use serde_json::json;
    let result = async {
        let thread: ThreadStartResponse = client.request_typed(ClientRequest::ThreadStart { request_id: RequestId::Integer(1), params: ThreadStartParams {
            ephemeral: Some(true), model: model.map(str::to_owned), service_name: Some(service.into()), approval_policy: Some(AskForApproval::Never), sandbox: Some(SandboxMode::ReadOnly), base_instructions: Some(instructions.into()), ..Default::default()
        }}).await?;
        let id = thread.thread.id;
        let started: TurnStartResponse = client.request_typed(serde_json::from_value(json!({
            "method": "turn/start", "id": 2, "params": {
                "threadId": id, "input": [{"type":"text","text":prompt}],
                "sandboxPolicy": {"type":"readOnly"}, "approvalPolicy":"never", "outputSchema": output_schema
            }
        }))?).await?;
        let mut answer = None;
        while let Some(event) = client.next_event().await {
            match event {
                AppServerEvent::ServerNotification(notification) => match *notification {
                    ServerNotification::ItemCompleted(item) if item.thread_id == id && item.turn_id == started.turn.id => {
                        if let ThreadItemPayload::AgentMessage { text, .. } = item.item.payload {
                            answer = Some(text);
                        }
                    }
                    ServerNotification::TurnCompleted(done) if done.thread_id == id && done.turn.id == started.turn.id => {
                        anyhow::ensure!(done.turn.status == TurnStatus::Completed, "Codex turn ended with {:?}", done.turn.status);
                        return answer.ok_or_else(|| anyhow::anyhow!("Codex completed without an agent message"));
                    }
                    _ => {}
                },
                AppServerEvent::ServerRequest(request) => {
                    client.reject_server_request(request.id().clone(), JSONRPCErrorError {
                        code: -32601, message: "Read-only generation does not provide client tools".into(), data: None,
                    }).await?;
                }
                AppServerEvent::Disconnected { .. } | AppServerEvent::Lagged { .. } => break,
                _ => {}
            }
        }
        anyhow::bail!("Codex event stream ended before turn completion")
    }.await;
    client.shutdown().await?;
    result
}


