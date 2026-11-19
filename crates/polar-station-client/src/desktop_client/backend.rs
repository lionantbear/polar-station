//! Backend selection extends the existing desktop worker; it never registers a
//! probe, steals a lease, migrates backend data, or replays an input operation.
use super::*;
use polar_station_protocol::{StationBackendAdmissionV1, StationBackendDescriptorV1};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendRequest {
    pub request_id: String,
    #[serde(flatten)]
    pub command: BackendCommand,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum BackendCommand {
    Inspect {
        socket_path: PathBuf,
        #[serde(default)]
        sources: Vec<polar_station_protocol::HostRefV1>,
    },
    Switch {
        preview_id: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendPreview {
    pub preview_id: String,
    pub socket_path: PathBuf,
    pub descriptor: StationBackendDescriptorV1,
    pub sources: Vec<polar_station_protocol::HostRefV1>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Endpoint {
    pub socket_path: PathBuf,
    pub host_id: Option<String>,
}
#[derive(Debug)]
pub(super) struct BackendState {
    pub preferences: preferences::Preferences,
    pub endpoint: Endpoint,
    pub preview: Option<BackendPreview>,
    pub switching: bool,
    pub pending: bool,
    pub initialized: bool,
    pub transitioning: bool,
}
impl BackendState {
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            preferences: preferences::Preferences::default(),
            endpoint: Endpoint {
                socket_path,
                host_id: None,
            },
            preview: None,
            switching: false,
            pending: false,
            initialized: true,
            transitioning: false,
        }
    }
}
impl DesktopClient {
    pub fn try_backend(&self, request: BackendRequest) -> Result<()> {
        polar_station_protocol::validate_stable_identifier(&request.request_id, "backend request")?;
        if let BackendCommand::Inspect {
            socket_path,
            sources,
        } = &request.command
        {
            anyhow::ensure!(sources.len() <= 256, "Too many Host references");
            for source in sources {
                source.validate()?;
            }
            anyhow::ensure!(
                socket_path.is_absolute() && socket_path.as_os_str().len() < 4096,
                "Select an absolute local Unix socket path"
            );
        }
        let mut backend = self.backend.lock().unwrap_or_else(|v| v.into_inner());
        anyhow::ensure!(backend.initialized, "Client preferences are not ready");
        anyhow::ensure!(!backend.pending, "A backend operation is already pending");
        backend.pending = true;
        backend.switching = matches!(request.command, BackendCommand::Switch { .. });
        let result = self
            .content
            .as_ref()
            .context("desktop client is not started")
            .and_then(|sender| {
                sender
                    .try_send(DesktopWorkerRequest::Backend(request))
                    .map_err(|_| anyhow::anyhow!("desktop worker is busy"))
            });
        if result.is_err() {
            backend.pending = false;
            backend.switching = false;
        }
        result
    }
    pub fn backend_socket_reader(&self) -> crate::dictate_client::EndpointReader {
        let backend=Arc::downgrade(&self.backend);let state=Arc::downgrade(&self.state);
        Arc::new(move || {
            let backend=backend.upgrade().context("Station client is closed")?;
            let backend=backend.lock().unwrap_or_else(|v|v.into_inner());
            let state=state.upgrade().context("Station client is closed")?;
            anyhow::ensure!(backend.initialized && !backend.switching && !backend.transitioning && state.lock().unwrap_or_else(|v|v.into_inner()).is_ready(),"Station backend is not ready");
            Ok(backend.endpoint.socket_path.clone())
        })
    }
    pub fn backend_socket(&self) -> PathBuf {
        self.backend
            .lock()
            .unwrap_or_else(|v| v.into_inner())
            .endpoint
            .socket_path
            .clone()
    }
}
pub(super) async fn inspect(
    path: PathBuf,
    instance: String,
    request_id: String,
    sources: Vec<polar_station_protocol::HostRefV1>,
) -> Result<BackendPreview> {
    tokio::time::timeout(IO_TIMEOUT, async {
        validate_socket_endpoint(&path)?;
        let mut stream = tokio::net::UnixStream::connect(&path).await?;
        anyhow::ensure!(
            stream.peer_cred()?.uid() == crate::local_ipc::effective_uid(),
            "Host peer belongs to another user"
        );
        let request = polar_station_protocol::LocalExtensionRequestV1 {
            extension: "polar.station.health".into(),
            version: 1,
            request_id: request_id.clone(),
            timeout_ms: 8_000,
            payload: serde_json::json!({"frontend_instance_id":instance,"sources":sources}),
        };
        crate::local_ipc::write_json_frame(
            &mut stream,
            &LocalClientFrame::ExtensionRequest { request },
        )
        .await?;
        let frame: LocalHostFrame = crate::local_ipc::read_json_frame(&mut stream)
            .await?
            .context("Host closed backend preview")?;
        let LocalHostFrame::ExtensionResponse { response } = frame else {
            bail!("Host does not support backend preview")
        };
        response.validate()?;
        anyhow::ensure!(
            response.extension == "polar.station.health"
                && response.version == 1
                && response.request_id == request_id,
            "Backend preview response mismatch"
        );
        let polar_station_protocol::LocalExtensionResultV1::Success { payload } = response.result
        else {
            bail!("Host refused backend preview")
        };
        let descriptor: StationBackendDescriptorV1 = serde_json::from_value(payload)
            .context("Update this Host to support backend selection")?;
        anyhow::ensure!(
            descriptor.backend_selection_version == 1
                && descriptor.desktop_version
                    == polar_station_protocol::DESKTOP_CONTROL_CONTRACT_VERSION,
            "Desktop and Host versions are incompatible"
        );
        anyhow::ensure!(
            descriptor.execution_uid == crate::local_ipc::effective_uid(),
            "Host runs as another user; configure that user as a remote Host"
        );
        anyhow::ensure!(
            descriptor.host.host_id == descriptor.host_id
                && descriptor.host.platform == polar_station_protocol::HostPlatform::current(),
            "Backend identity mismatch"
        );
        polar_station_protocol::validate_stable_identifier(&descriptor.host_id, "backend Host")?;
        Ok(BackendPreview {
            preview_id: request_id,
            socket_path: path,
            descriptor,
            sources,
        })
    })
    .await
    .context("Backend preview timed out; current connection is unchanged")?
}
pub(super) fn finish(
    request: BackendRequest,
    result: Result<BackendPreview>,
    backend: &Arc<Mutex<BackendState>>,
    state: &Arc<Mutex<DesktopClientStateMachine>>,
    retry: &mpsc::Sender<()>,
    idle: bool,
) -> DesktopClientEvent {
    let mut backend = backend.lock().unwrap_or_else(|v| v.into_inner());
    let switching = matches!(request.command, BackendCommand::Switch { .. });
    let result = result.and_then(|preview| {
        if switching {
            let original = backend
                .preview
                .as_ref()
                .context("Preview the selected backend again")?;
            anyhow::ensure!(
                original.socket_path == preview.socket_path
                    && original.descriptor == preview.descriptor,
                "Backend facts changed; preview again before switching"
            );
            anyhow::ensure!(
                preview.descriptor.admission != StationBackendAdmissionV1::Occupied,
                "Another client is using this backend; current connection is unchanged"
            );
            let mut state = state.lock().unwrap_or_else(|v| v.into_inner());
            anyhow::ensure!(
                idle && state.requests.len() == 0,
                "Wait for current operations to finish before switching; no operation was replayed"
            );
            let permit = retry
                .try_reserve()
                .context("Connection worker is busy; retry switching")?;
            // Invalidate the old generation before publishing a new endpoint.
            // Old reader/outbound events cannot become ready again in this gap.
            let _ = state.begin_connection(1);
            state.expected_host_id = Some(preview.descriptor.host_id.clone());
            backend.transitioning = true;
            backend.endpoint = Endpoint {
                socket_path: preview.socket_path.clone(),
                host_id: Some(preview.descriptor.host_id.clone()),
            };
            permit.send(());
        } else {
            backend.preview = Some(preview.clone());
        }
        Ok(preview)
    });
    backend.pending = false;
    backend.switching = false;
    match result {
        Ok(preview) => DesktopClientEvent::BackendCompleted {
            request_id: request.request_id,
            switched: switching,
            preview: Some(preview),
            error: None,
        },
        Err(error) => DesktopClientEvent::BackendCompleted {
            request_id: request.request_id,
            switched: false,
            preview: None,
            error: Some(format!("{error:#}")),
        },
    }
}


