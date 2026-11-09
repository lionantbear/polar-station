//! Shared generation-fenced desktop control client.
//!
//! This module is the only desktop owner of connection readiness and inbound
//! generation fencing. Native adapters drain typed events and never infer
//! readiness from a physical Unix-socket connection.

use std::{
    collections::VecDeque,
    fs,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use polar_tmux_protocol::{
    DesktopSendError, LocalClientFrame, LocalHostFrame, PaneContentRequestV1, PaneContentResponseV1,
    PaneContentResultV1, ProtocolError, PROTOCOL_VERSION,
};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWrite;
use tokio::sync::{mpsc, watch, Notify};

mod requests;
pub use requests::RequestFailure;

const IO_TIMEOUT: Duration = Duration::from_secs(8);
const INITIAL_RECONNECT_DELAY: Duration = Duration::from_millis(250);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DesktopClientEvent {
    SessionCompleted {request_id:String,value:Option<serde_json::Value>,error:Option<String>},
    RequestFailed { generation: u64, failure: RequestFailure },
    NativeStateCompleted {
        revision: u64,
        document: Option<String>,
        error: Option<String>,
    },
    ContentCompleted {
        response: PaneContentResponseV1,
    },
    Connecting {
        generation: u64,
        attempt: u32,
    },
    Ready {
        generation: u64,
        registration_id: String,
        host_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_control_versions: Option<polar_tmux_protocol::HostControlVersionsV1>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        extensions: Vec<polar_tmux_protocol::LocalExtensionOfferV1>,
    },
    Frame {
        generation: u64,
        frame: Box<LocalHostFrame>,
    },
    Unavailable {
        generation: u64,
        attempt: u32,
        message: String,
        retry_after_ms: u64,
    },
}

/// Pure connection state used by the socket worker and independently tested.
#[derive(Debug)]
pub struct DesktopClientStateMachine {
    generation: u64,
    registered: bool,
    requests: requests::Requests,
    offered_extensions: Vec<polar_tmux_protocol::LocalExtensionOfferV1>,
    negotiated_extensions: Vec<polar_tmux_protocol::LocalExtensionOfferV1>,
    capacity: usize,
    events: VecDeque<DesktopClientEvent>,
    content_events: VecDeque<DesktopClientEvent>,
    request_events: VecDeque<DesktopClientEvent>,
}

impl DesktopClientStateMachine {
    pub fn new(capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "desktop client queue capacity must be positive"
        );
        Self {
            generation: 0,
            registered: false,
            requests: requests::Requests::default(),
            offered_extensions: Vec::new(),
            negotiated_extensions: Vec::new(),
            capacity,
            events: VecDeque::with_capacity(capacity),
            content_events: VecDeque::with_capacity(capacity),
            request_events: VecDeque::new(),
        }
    }

    pub fn new_with_extensions(
        capacity: usize, offered: Vec<polar_tmux_protocol::LocalExtensionOfferV1>,
    ) -> Result<Self> {
        polar_tmux_protocol::validate_extension_offers(&offered)?;
        let mut state = Self::new(capacity);
        state.offered_extensions = offered;
        Ok(state)
    }

    pub fn validate_outbound_extension(&self, frame: &LocalClientFrame) -> Result<()> {
        let (name, version) = match frame {
            LocalClientFrame::ExtensionRequest { request } => (&request.extension, request.version),
            LocalClientFrame::ExtensionCancel { request } => (&request.extension, request.version),
            _ => return Ok(()),
        };
        if !self.registered { bail!("desktop client is not registered"); }
        let grant = self.negotiated_extensions.iter().find(|g| g.permits(name, version))
            .context("the Host did not negotiate this extension")?;
        if let LocalClientFrame::ExtensionRequest { request } = frame {
            request.validate_with_limit(grant.max_payload_bytes as usize)?;
        }
        Ok(())
    }

    pub fn validate_inbound_extension(&self, frame: &LocalHostFrame) -> Result<()> {
        if let LocalHostFrame::ExtensionEvent { event } = frame {
            if !self.registered { bail!("extension event preceded registration"); }
            let grant = self.negotiated_extensions.iter()
                .find(|g| g.permits(&event.extension, event.version))
                .context("Host sent an unnegotiated extension event")?;
            event.validate_with_limit(grant.max_payload_bytes as usize)?;
        }
        if let LocalHostFrame::ExtensionResponse { response } = frame {
            if !self.registered { bail!("extension response preceded registration"); }
            let grant = self.negotiated_extensions.iter()
                .find(|g| g.permits(&response.extension, response.version))
                .context("Host sent an unnegotiated extension response")?;
            response.validate_with_limit(grant.max_payload_bytes as usize)?;
        }
        Ok(())
    }

    #[must_use]
    pub fn begin_connection(&mut self, attempt: u32) -> u64 {
        self.generation = self.generation.wrapping_add(1).max(1);
        self.registered = false;
        self.requests = requests::Requests::default();
        self.negotiated_extensions.clear();
        self.events.clear();
        let generation = self.generation;
        let _ = self.push(DesktopClientEvent::Connecting {
            generation,
            attempt,
        });
        generation
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.registered
    }

    #[must_use]
    pub fn current_generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn accept_host_frame(&mut self, generation: u64, frame: LocalHostFrame) -> bool {
        if generation != self.generation {
            return false;
        }
        match frame {
            LocalHostFrame::Registered {
                registration_id,
                protocol_version,
                host_id,
                host_control_versions,
                extensions,
            } if !self.registered && protocol_version == PROTOCOL_VERSION => {
                if polar_tmux_protocol::validate_extension_ack(&self.offered_extensions, &extensions).is_err() {
                    return false;
                }
                let negotiated = extensions.clone();
                let accepted = self.push(DesktopClientEvent::Ready {
                    generation,
                    registration_id,
                    host_id,
                    host_control_versions,
                    extensions,
                });
                if accepted {
                    self.registered = true;
                    self.negotiated_extensions = negotiated;
                }
                accepted
            }
            LocalHostFrame::Registered { .. } => false,
            frame if self.registered => {
                if self.validate_inbound_extension(&frame).is_err() { return false; }
                self.push(DesktopClientEvent::Frame { generation, frame: Box::new(frame) })
            },
            _ => false,
        }
    }

    pub fn connection_unavailable(
        &mut self,
        generation: u64,
        attempt: u32,
        message: impl Into<String>,
        retry_after: Duration,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        self.registered = false;
        self.negotiated_extensions.clear();
        self.events.clear();
        let message = message.into();
        for failure in self.requests.disconnect(&message) {
            let _ = self.push(DesktopClientEvent::RequestFailed { generation, failure });
        }
        self.push(DesktopClientEvent::Unavailable {
            generation,
            attempt,
            message,
            retry_after_ms: duration_millis(retry_after),
        })
    }

    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.events.len() + self.content_events.len() + self.request_events.len()
    }

    pub fn pop_event(&mut self) -> Option<DesktopClientEvent> {
        self.request_events.pop_front().or_else(|| self.content_events
            .pop_front())
            .or_else(|| self.events.pop_front())
    }

    fn push(&mut self, event: DesktopClientEvent) -> bool {
        if matches!(&event, DesktopClientEvent::RequestFailed { .. }) || matches!(&event,
            DesktopClientEvent::Frame { frame, .. } if matches!(frame.as_ref(), LocalHostFrame::Response { .. } | LocalHostFrame::ApplicationResponse { .. })) {
            if self.request_events.len() >= requests::LIMIT { return false; }
            self.request_events.push_back(event);
            return true;
        }
        if self.events.len() == self.capacity {
            return false;
        }
        self.events.push_back(event);
        true
    }

    fn replace_with_unavailable(
        &mut self,
        generation: u64,
        attempt: u32,
        message: String,
        retry_after: Duration,
    ) {
        if generation != self.generation {
            return;
        }
        self.registered = false;
        self.negotiated_extensions.clear();
        self.events.clear();
        for failure in self.requests.disconnect(&message) {
            let _ = self.push(DesktopClientEvent::RequestFailed { generation, failure });
        }
        let _ = self.push(DesktopClientEvent::Unavailable {
            generation,
            attempt,
            message,
            retry_after_ms: duration_millis(retry_after),
        });
    }
}

fn duration_millis(value: Duration) -> u64 {
    u64::try_from(value.as_millis()).unwrap_or(u64::MAX)
}

fn default_event_capacity() -> usize {
    256
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopClientConfig {
    pub socket_path: PathBuf,
    pub registration: LocalClientFrame,
    #[serde(default = "default_event_capacity")]
    pub event_capacity: usize,
}

impl DesktopClientConfig {
    pub fn validate(&self) -> Result<()> {
        if self.socket_path.as_os_str().is_empty() {
            bail!("desktop client socket path must not be empty");
        }
        if self.event_capacity == 0 || self.event_capacity > 4096 {
            bail!("desktop client event capacity must be between 1 and 4096");
        }
        self.registration
            .validate()
            .context("invalid desktop registration")?;
        if !matches!(self.registration, LocalClientFrame::Register { .. }) {
            bail!("desktop client registration must be a register frame");
        }
        Ok(())
    }
}

type Wake = Arc<dyn Fn() + Send + Sync + 'static>;

/// Content I/O contains only an opaque channel and its requested Session.
/// It is independent of Host registration and never changes Session existence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopContentRequest {
    pub socket_path: PathBuf,
    pub request: PaneContentRequestV1,
}

/// Background control client shared by the macOS and Linux presentation
/// adapters. Starting it is idempotent; it owns exactly one reconnect worker.
/// Native-only persistence job; never sent to the Host.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopNativeStateRequest {
    pub path: PathBuf,
    pub revision: u64,
    pub document: Option<String>,
}

enum DesktopWorkerRequest {
    Session(crate::station_input_client::DesktopSessionRequest),
    Content(DesktopContentRequest),
    NativeState(DesktopNativeStateRequest),
}

pub struct DesktopClient {
    config: DesktopClientConfig,
    state: Arc<Mutex<DesktopClientStateMachine>>,
    wake: Wake,
    outbound: Option<mpsc::Sender<(u64, LocalClientFrame)>>,
    content: Option<mpsc::Sender<DesktopWorkerRequest>>,
    content_drained: Arc<Notify>,
    retry: Option<mpsc::Sender<()>>,
    stop: Option<watch::Sender<bool>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl DesktopClient {
    pub fn new(
        config: DesktopClientConfig,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self> {
        crate::operation_trace::init();
        tracing::info!(target: "polar_station_operation", component="Desktop", event="desktop_client_created");
        config.validate()?;
        Ok(Self {
            state: Arc::new(Mutex::new(DesktopClientStateMachine::new_with_extensions(
                config.event_capacity,
                match &config.registration {
                    LocalClientFrame::Register { extensions, .. } => extensions.clone(),
                    _ => Vec::new(), // Config validation above rejects this case.
                },
            )?)),
            config,
            wake: Arc::new(wake),
            outbound: None,
            content: None,
            content_drained: Arc::new(Notify::new()),
            retry: None,
            stop: None,
            worker: None,
        })
    }

    pub fn start(&mut self) -> Result<()> {
        if self.worker.is_some() {
            return Ok(());
        }
        let (outbound, outbound_rx) = mpsc::channel(64);
        let (content, content_rx) = mpsc::channel(64);
        let (retry, retry_rx) = mpsc::channel(1);
        let (stop, stop_rx) = watch::channel(false);
        let config = self.config.clone();
        let state = Arc::clone(&self.state);
        let wake = Arc::clone(&self.wake);
        let content_drained = self.content_drained.clone();
        let worker = thread::Builder::new()
            .name("polar-station-desktop-client".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => runtime.block_on(async {
                        // Both lanes run on the existing one desktop thread.
                        // A stalled content endpoint cannot block Host events.
                        tokio::join!(
                            run_content_worker(
                                content_rx,
                                state.clone(),
                                wake.clone(),
                                content_drained,
                                config.socket_path.clone(),
                                stop_rx.clone()
                            ),
                            run_worker(config, state, wake, outbound_rx, retry_rx, stop_rx),
                        );
                    }),
                    Err(error) => {
                        let mut state = state.lock().unwrap_or_else(|value| value.into_inner());
                        let generation = state.current_generation();
                        state.replace_with_unavailable(
                            generation,
                            0,
                            format!("desktop client runtime failed: {error}"),
                            Duration::ZERO,
                        );
                        wake();
                    }
                }
            })
            .context("failed to start desktop client thread")?;
        self.outbound = Some(outbound);
        self.content = Some(content);
        self.retry = Some(retry);
        self.stop = Some(stop);
        self.worker = Some(worker);
        Ok(())
    }

    pub fn try_send(&self, frame: LocalClientFrame) -> Result<()> {
        self.try_send_detailed(frame).map_err(Into::into)
    }

    pub fn try_send_detailed(&self, frame: LocalClientFrame) -> std::result::Result<(), DesktopSendError> {
        let operation = match &frame {
            LocalClientFrame::ExtensionRequest { .. } => "extension_request",
            LocalClientFrame::ExtensionCancel { .. } => "extension_cancel",
            LocalClientFrame::ApplicationRequest { .. } => "application_request",
            _ => "control",
        };
        self.admit_outbound(frame).inspect_err(|reason| {
            let generation = self.state.lock().unwrap_or_else(|value| value.into_inner()).current_generation();
            tracing::warn!(target: "polar_station_operation", component="Desktop", event="request_rejected", operation, generation, reason=?reason);
        })
    }

    fn admit_outbound(&self, frame: LocalClientFrame) -> std::result::Result<(), DesktopSendError> {
        frame.validate().map_err(|_| DesktopSendError::InvalidRequest)?;
        if matches!(frame, LocalClientFrame::Register { .. }) {
            return Err(DesktopSendError::InvalidRequest);
        }
        let Some(outbound) = &self.outbound else {
            return Err(DesktopSendError::Unavailable);
        };
        let mut state = self.state.lock().unwrap_or_else(|value| value.into_inner());
        if !state.is_ready() { return Err(DesktopSendError::NotReady); }
        let extension = match &frame {
            LocalClientFrame::ExtensionRequest { request } => Some((&request.extension, request.version)),
            LocalClientFrame::ExtensionCancel { request } => Some((&request.extension, request.version)),
            _ => None,
        };
        if let Some((name, version)) = extension {
            if !state.negotiated_extensions.iter().any(|g| g.permits(name, version)) {
                return Err(DesktopSendError::Unsupported);
            }
        }
        state.validate_outbound_extension(&frame).map_err(|_| DesktopSendError::InvalidRequest)?;
        if state.requests.len() + state.request_events.len() >= requests::LIMIT { return Err(DesktopSendError::QueueFull); }
        let id = state.requests.admit(&frame, tokio::time::Instant::now())?;
        let generation = state.current_generation();
        let result = outbound.try_send((generation, frame)).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => DesktopSendError::QueueFull,
            mpsc::error::TrySendError::Closed(_) => DesktopSendError::Unavailable,
        });
        if result.is_err() { if let Some(id) = id { state.requests.rollback(&id); } }
        result
    }

    pub fn cancel_request(&self, generation: u64, request_id: &str) {
        let mut state = self.state.lock().unwrap_or_else(|value| value.into_inner());
        if state.current_generation() != generation { return; }
        if let Some(failure) = state.requests.cancel(request_id) {
            if !state.push(DesktopClientEvent::RequestFailed { generation, failure }) {
                state.replace_with_unavailable(generation, 1, "Request completion lane overflow".into(), INITIAL_RECONNECT_DELAY);
            }
            drop(state);
            (self.wake)();
        }
    }

    pub fn pop_event(&self) -> Option<DesktopClientEvent> {
        let event = self
            .state
            .lock()
            .unwrap_or_else(|value| value.into_inner())
            .pop_event();
        if event.is_some() {
            self.content_drained.notify_one();
        }
        event
    }

    pub fn try_send_content(&self, request: DesktopContentRequest) -> Result<()> {
        request
            .request
            .validate()
            .context("invalid content request")?;
        if request.socket_path.as_os_str().is_empty() {
            bail!("content socket path must not be empty");
        }
        self.content
            .as_ref()
            .context("desktop client is not started")?
            .try_send(DesktopWorkerRequest::Content(request))
            .map_err(|_| anyhow::anyhow!("content queue is busy"))
    }

    pub(crate) fn try_session(&self, request: crate::station_input_client::DesktopSessionRequest) -> Result<()> {
        polar_tmux_protocol::validate_stable_identifier(&request.request_id,"session operation")?;
        self.content.as_ref().context("desktop client is not started")?
            .try_send(DesktopWorkerRequest::Session(request))
            .map_err(|_|anyhow::anyhow!("desktop worker is busy"))
    }

    pub fn try_native_state(&self, request: DesktopNativeStateRequest) -> Result<()> {
        if !request.path.is_absolute()
            || request
                .path
                .file_name()
                .is_none_or(|name| name != "native-layout.json")
            || request
                .document
                .as_ref()
                .is_some_and(|value| value.len() > 1024 * 1024)
        {
            bail!("invalid native state job");
        }
        self.content
            .as_ref()
            .context("desktop client is not started")?
            .try_send(DesktopWorkerRequest::NativeState(request))
            .map_err(|_| anyhow::anyhow!("native state queue is busy"))
    }

    pub fn retry_now(&self) -> Result<()> {
        let Some(retry) = &self.retry else {
            bail!("desktop client is not started");
        };
        match retry.try_send(()) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(())) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(())) => {
                bail!("desktop client retry channel is unavailable")
            }
        }
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|value| value.into_inner())
            .is_ready()
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.state.lock().unwrap_or_else(|value| value.into_inner()).current_generation()
    }
    pub fn has_pending(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|value| value.into_inner())
            .pending_len()
            > 0
    }

    pub fn stop(&mut self) {
        self.outbound = None;
        self.content = None;
        self.retry = None;
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(true);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for DesktopClient {
    fn drop(&mut self) {
        self.stop();
    }
}

fn reject_content_request(request: &PaneContentRequestV1, code: &str, message: &str, state: &Arc<Mutex<DesktopClientStateMachine>>, wake: &Wake) {
    let response = PaneContentResponseV1 { protocol_version: PROTOCOL_VERSION, api: request.api,
        request_id: request.request_id.clone(), channel_id: request.channel_id.clone(),
        channel_generation: request.channel_generation, content_revision: request.content_revision,
        result: PaneContentResultV1::Rejected { error: ProtocolError::new(code, message, false) } };
    state.lock().unwrap_or_else(|v| v.into_inner()).content_events.push_back(DesktopClientEvent::ContentCompleted { response });
    wake();
}

enum ContentWorkerResult {
    Content {key:(String,u64,u8), revision:u64, response:PaneContentResponseV1},
    Session(DesktopClientEvent),
}

async fn run_content_worker(
    mut incoming: mpsc::Receiver<DesktopWorkerRequest>,
    state: Arc<Mutex<DesktopClientStateMachine>>,
    wake: Wake,
    drained: Arc<Notify>,
    host_socket: PathBuf,
    mut stop: watch::Receiver<bool>,
) {
    use std::collections::BTreeMap;
    let mut jobs = tokio::task::JoinSet::new();
    let mut persistence = tokio::task::JoinSet::new();
    let mut pending_native = std::collections::VecDeque::new();
    let mut current = BTreeMap::<(String, u64, u8), (u64, tokio::task::AbortHandle, Option<PaneContentRequestV1>)>::new();
    loop {
        // Do not lose a completion when the renderer is temporarily behind.
        // Stop is still serviced; bounded admitted jobs retain their result.
        let room = {
            let state = state.lock().unwrap_or_else(|v| v.into_inner());
            state.content_events.len() < state.capacity
        };
        tokio::select! {
            _ = stop.changed() => break,
            value = incoming.recv(), if room => {
                let Some(value) = value else { break; };
                let job = match value {
                    DesktopWorkerRequest::Content(job) => job,
                    DesktopWorkerRequest::Session(job) => {
                        if jobs.len() >= 64 {
                            state.lock().unwrap_or_else(|v|v.into_inner()).content_events.push_back(DesktopClientEvent::SessionCompleted {
                                request_id:job.request_id,value:None,error:Some("Desktop worker capacity reached; operation not sent".into())});
                            wake();
                        } else {
                            let host_socket=host_socket.clone();
                            jobs.spawn(async move {ContentWorkerResult::Session(crate::station_input_client::execute(job,host_socket).await)});
                        }
                        continue;
                    },
                    DesktopWorkerRequest::NativeState(job) => {
                        if persistence.is_empty() { persistence.spawn_blocking(move || native_state_io(job)); }
                        else { pending_native.push_back(job); }
                        continue;
                    },
                };
                use polar_tmux_protocol::PaneContentCommandV1 as C;
                if matches!(job.request.command, C::UnobserveRecovery) {
                    if let Some((_, task, _)) = current.remove(&(job.request.channel_id.clone(), job.request.channel_generation, 1)) { task.abort(); }
                    continue;
                }
                let lane = match job.request.command { C::ObserveRecovery => 1, C::ProbeRecovery => 2, C::DismissRecovery => 3, C::Focus { .. } | C::TerminalControl { .. } => 4, _ => 0 };
                let key = (job.request.channel_id.clone(), job.request.channel_generation, lane);
                if matches!(job.request.command, C::Retire) {
                    if let Some((_, task, _)) = current.remove(&(key.0.clone(), key.1, 1)) { task.abort(); }
                }
                let revision = match job.request.command { C::Focus { sequence, .. } | C::TerminalControl { sequence, .. } => sequence, _ => job.request.content_revision };
                if let Some((previous_revision, handle, previous_request)) = current.get(&key) {
                    if revision <= *previous_revision {
                        if lane == 0 { reject_content_request(&job.request, "stale_content_request", "A newer terminal binding already exists. This request was not sent.", &state, &wake); }
                        continue;
                    }
                    handle.abort();
                    if let Some(request) = previous_request {
                        reject_content_request(request, "content_outcome_unknown", "A newer terminal binding superseded this request. The earlier operation may have completed; inspect the current binding before retrying.", &state, &wake);
                    }
                }
                if current.len() >= 64 && !current.contains_key(&key) {
                    let response = PaneContentResponseV1 {
                        protocol_version: PROTOCOL_VERSION, api: job.request.api,
                        request_id: job.request.request_id, channel_id: job.request.channel_id,
                        channel_generation: job.request.channel_generation, content_revision: revision,
                        result: PaneContentResultV1::Rejected { error: polar_tmux_protocol::ProtocolError::new(
                            "content_capacity", "The terminal control queue is full. Retry this operation.", true) },
                    };
                    state.lock().unwrap_or_else(|v| v.into_inner()).content_events.push_back(DesktopClientEvent::ContentCompleted { response });
                    wake();
                    continue;
                }
                let return_key = key.clone();
                let receipt = (lane == 0).then(|| job.request.clone());
                let watch_state = state.clone();
                let watch_wake = wake.clone();
                let watch_drained = drained.clone();
                let handle = jobs.spawn(async move {
                    let response = if lane == 1 { observe_content_recovery(&job, watch_state, watch_wake, watch_drained).await } else { exchange_content(&job).await };
                    ContentWorkerResult::Content {key:return_key, revision, response}
                });
                current.insert(key, (revision, handle, receipt));
            }
            Some(result) = jobs.join_next(), if !jobs.is_empty() && room => {
                match result {
                    Ok(ContentWorkerResult::Session(event)) => {
                        state.lock().unwrap_or_else(|v|v.into_inner()).content_events.push_back(event);
                        wake();
                    }
                    Ok(ContentWorkerResult::Content {key, revision, response}) => {
                    if current.get(&key).is_some_and(|(latest, _, _)| *latest == revision) {
                        current.remove(&key);
                        state.lock().unwrap_or_else(|v| v.into_inner()).content_events
                            .push_back(DesktopClientEvent::ContentCompleted { response });
                        wake();
                    }
                    }
                    Err(_) => {},
                }
            }
            Some(result) = persistence.join_next(), if !persistence.is_empty() && room => {
                if let Ok(event) = result {
                    state.lock().unwrap_or_else(|v| v.into_inner()).content_events.push_back(event);
                    wake();
                }
                if let Some(job) = pending_native.pop_front() { persistence.spawn_blocking(move || native_state_io(job)); }
            }
            _ = drained.notified(), if !room => {},
        }
    }
    // Drain every already-admitted snapshot on shutdown in admission order.
    // Each accepted transaction retains its completion. Content jobs are cancelled rather than touching Sessions.
    while let Ok(request) = incoming.try_recv() {
        if let DesktopWorkerRequest::NativeState(job) = request {
            pending_native.push_back(job);
        }
    }
    jobs.abort_all();
    while jobs.join_next().await.is_some() {}
    // Finish the admitted disk write before the final newer snapshot. No disk
    // work runs on a native UI thread or blocks the Host control lane.
    while persistence.join_next().await.is_some() {}
    while let Some(job) = pending_native.pop_front() {
        let _ = tokio::task::spawn_blocking(move || native_state_io(job)).await;
    }
}

fn native_state_io(job: DesktopNativeStateRequest) -> DesktopClientEvent {
    let result = crate::native_state_file::exchange(&job.path, job.document.as_deref(), 1024 * 1024);
    match result {
        Ok(document) => DesktopClientEvent::NativeStateCompleted {
            revision: job.revision,
            document,
            error: None,
        },
        Err(_) => DesktopClientEvent::NativeStateCompleted {
            revision: job.revision,
            document: None,
            error: Some("Native layout persistence failed; tmux Sessions are unchanged".into()),
        },
    }
}

/// Long-lived read subscription on the existing content endpoint. Intermediate
/// projections coalesce by channel; terminal bytes never enter this lane.
async fn publish_recovery_event(
    response: PaneContentResponseV1,
    state: &Arc<Mutex<DesktopClientStateMachine>>,
    wake: &Wake,
    drained: &Notify,
) {
    loop {
        let notified = drained.notified();
        {
            let mut state = state.lock().unwrap_or_else(|v| v.into_inner());
            state.content_events.retain(|event| !matches!(event, DesktopClientEvent::ContentCompleted { response: previous }
                if previous.channel_id == response.channel_id && previous.channel_generation == response.channel_generation
                && matches!(previous.result, PaneContentResultV1::Recovery { .. })));
            if state.content_events.len() < state.capacity {
                state
                    .content_events
                    .push_back(DesktopClientEvent::ContentCompleted { response });
                drop(state);
                wake();
                return;
            }
        }
        notified.await;
    }
}

async fn observe_content_recovery(
    job: &DesktopContentRequest,
    state: Arc<Mutex<DesktopClientStateMachine>>,
    wake: Wake,
    drained: Arc<Notify>,
) -> PaneContentResponseV1 {
    let mut delay = INITIAL_RECONNECT_DELAY;
    // Hidden native Tabs may not start their Pane Host for minutes. Arm the
    // existing OS socket notification before attempting the connection; its
    // publication wakes this observer even after the retry budget grew.
    let mut readiness =
        crate::local_ipc::socket_readiness::SocketReadiness::new(&job.socket_path).ok();
    let mut revision = job.request.content_revision;
    loop {
        let mut unavailable = polar_tmux_core::terminal_feedback(
            polar_tmux_protocol::TerminalRecoveryPhaseV1::StatusUnavailable,
            None,
        );
        unavailable.content_generation = revision;
        let response = PaneContentResponseV1 {
            protocol_version: PROTOCOL_VERSION,
            api: job.request.api,
            request_id: job.request.request_id.clone(),
            channel_id: job.request.channel_id.clone(),
            channel_generation: job.request.channel_generation,
            content_revision: revision,
            result: PaneContentResultV1::Recovery {
                view: Box::new(unavailable),
            },
        };
        publish_recovery_event(response, &state, &wake, &drained).await;
        let result: Result<()> = async {
            tracing::info!(target: "polar_station_operation", component="NativeIPC", event="content_recovery_connect", consumer_id=job.request.channel_id.as_str());
            validate_socket_endpoint(&job.socket_path)?;
            let mut stream = tokio::time::timeout(
                IO_TIMEOUT,
                tokio::net::UnixStream::connect(&job.socket_path),
            )
            .await??;
            tokio::time::timeout(
                IO_TIMEOUT,
                crate::local_ipc::write_json_frame(&mut stream, &job.request),
            )
            .await??;
            tracing::info!(target: "polar_station_operation", component="NativeIPC", event="content_recovery_connected", consumer_id=job.request.channel_id.as_str());
            let mut initial = true;
            loop {
                let read = crate::local_ipc::read_json_frame(&mut stream);
                let mut response: PaneContentResponseV1 = if initial {
                    tokio::time::timeout(IO_TIMEOUT, read).await??
                } else {
                    read.await?
                }
                .context("recovery subscription closed")?;
                initial = false;
                response.validate()?;
                anyhow::ensure!(
                    response.channel_id == job.request.channel_id
                        && response.channel_generation == job.request.channel_generation
                        && response.request_id == job.request.request_id,
                    "recovery response identity mismatch"
                );
                let PaneContentResultV1::Recovery { view } = &mut response.result else {
                    anyhow::bail!("recovery subscription unsupported");
                };
                if response.content_revision < revision {
                    continue;
                }
                revision = response.content_revision;
                view.input_warning = false;
                view.input_warning_message.clear();
                if view.phase == polar_tmux_protocol::TerminalRecoveryPhaseV1::Hidden {
                    view.visible = false;
                }
                delay = INITIAL_RECONNECT_DELAY;
                publish_recovery_event(response, &state, &wake, &drained).await;
            }
        }
        .await;
        tracing::debug!(
            failed = result.is_err(),
            channel = job.request.channel_id,
            "terminal status subscription reconnecting"
        );
        // Publish failure before the retry delay; stale Ready must never remain displayed.
        let mut view = polar_tmux_core::terminal_feedback(
            polar_tmux_protocol::TerminalRecoveryPhaseV1::StatusUnavailable,
            None,
        );
        view.content_generation = revision;
        publish_recovery_event(
            PaneContentResponseV1 {
                protocol_version: PROTOCOL_VERSION,
                api: job.request.api,
                request_id: job.request.request_id.clone(),
                channel_id: job.request.channel_id.clone(),
                channel_generation: job.request.channel_generation,
                content_revision: revision,
                result: PaneContentResultV1::Recovery {
                    view: Box::new(view),
                },
            },
            &state,
            &wake,
            &drained,
        )
        .await;
        if let Some(notification) = &readiness {
            if wait_for_content_publication(notification, job, delay)
                .await
                .is_err()
            {
                readiness = None;
                tokio::time::sleep(delay).await;
            }
        } else {
            tokio::time::sleep(delay).await;
        }
        delay = (delay * 2).min(MAX_RECONNECT_DELAY);
    }
}

async fn wait_for_content_publication(
    readiness: &crate::local_ipc::socket_readiness::SocketReadiness,
    job: &DesktopContentRequest,
    delay: Duration,
) -> std::io::Result<()> {
    let deadline = tokio::time::Instant::now() + delay;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => return Ok(()),
            changed = readiness.changed() => {
                changed?;
                // Directory events can describe another Pane or a non-socket.
                // Preserve the existing exact endpoint ownership/mode checks.
                if validate_socket_endpoint(&job.socket_path).is_ok() {
                    tracing::info!(target: "polar_station_operation", component="NativeIPC", event="content_endpoint_published", consumer_id=job.request.channel_id.as_str());
                    return Ok(());
                }
            }
        }
    }
}

async fn exchange_content(job: &DesktopContentRequest) -> PaneContentResponseV1 {
    let focus = match &job.request.command {
        polar_tmux_protocol::PaneContentCommandV1::Focus { sequence, focused }
        | polar_tmux_protocol::PaneContentCommandV1::TerminalControl { sequence, focused, .. } => Some((*sequence, *focused)),
        _ => None,
    };
    let span = if let Some((sequence, focused)) = focus {
        tracing::info_span!(target: "polar_station_operation", "native_focus_exchange", component="NativeIPC", consumer_id=job.request.channel_id.as_str(), sequence, focused, operation_id=format!("{}-{sequence}", job.request.channel_id), outcome="cancelled")
    } else { tracing::Span::none() };
    let exchange = async {
        validate_socket_endpoint(&job.socket_path)?;
        let mut stream = tokio::net::UnixStream::connect(&job.socket_path).await?;
        crate::local_ipc::write_json_frame(&mut stream, &job.request).await?;
        let response: PaneContentResponseV1 = crate::local_ipc::read_json_frame(&mut stream)
            .await?
            .context("content endpoint closed")?;
        response.validate()?;
        anyhow::ensure!(
            response.request_id == job.request.request_id
                && response.channel_id == job.request.channel_id
                && response.channel_generation == job.request.channel_generation
                && response.content_revision == job.request.content_revision,
            "content response identity mismatch"
        );
        Ok::<_, anyhow::Error>(response)
    };
    let result = tokio::time::timeout(IO_TIMEOUT, exchange).await;
    let endpoint_gone = matches!(&result, Ok(Err(error)) if error.downcast_ref::<std::io::Error>()
        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound));
    let response = match result {
        Ok(Ok(response)) => response,
        _ => PaneContentResponseV1 {
            protocol_version: PROTOCOL_VERSION,
            api: job.request.api,
            request_id: job.request.request_id.clone(),
            channel_id: job.request.channel_id.clone(),
            channel_generation: job.request.channel_generation,
            content_revision: job.request.content_revision,
            result: PaneContentResultV1::Rejected {
                error: if endpoint_gone {
                    ProtocolError::new(
                        "content_endpoint_gone",
                        "The terminal connection has closed; the Session has not been changed.",
                        false,
                    )
                } else {
                    ProtocolError::new(
                        "content_unavailable",
                        "The terminal connection is unavailable. Retry this connection; the Session has not been changed.",
                        true,
                    )
                },
            },
        },
    };
    span.record(
        "outcome",
        if matches!(response.result, PaneContentResultV1::Rejected { .. }) {
            "rejected"
        } else {
            "ok"
        },
    );
    response
}

async fn run_worker(
    config: DesktopClientConfig,
    state: Arc<Mutex<DesktopClientStateMachine>>,
    wake: Wake,
    mut outbound: mpsc::Receiver<(u64, LocalClientFrame)>,
    mut retry: mpsc::Receiver<()>,
    mut stop: watch::Receiver<bool>,
) {
    let mut reconnect_delay = INITIAL_RECONNECT_DELAY;
    let mut attempt = 0_u32;
    while !*stop.borrow() {
        attempt = attempt.saturating_add(1).max(1);
        let generation = {
            let mut state = state.lock().unwrap_or_else(|value| value.into_inner());
            state.begin_connection(attempt)
        };
        wake();
        if let Err(error) = validate_socket_endpoint(&config.socket_path) {
            publish_unavailable(
                &state,
                &wake,
                generation,
                attempt,
                format!("Host unavailable: {error}"),
                reconnect_delay,
            );
            match wait_for_retry(reconnect_delay, &mut retry, &mut stop).await {
                WorkerSignal::Stop => break,
                WorkerSignal::Retry => continue,
                WorkerSignal::Elapsed => {
                    reconnect_delay = (reconnect_delay * 2).min(MAX_RECONNECT_DELAY);
                    continue;
                }
            }
        }
        tracing::info!(target: "polar_station_operation", component="Desktop", event="ipc_connect", generation, attempt);
        let connect = tokio::net::UnixStream::connect(&config.socket_path);
        let stream = tokio::select! {
            _ = stop.changed() => break,
            value = retry.recv() => {
                if value.is_none() { break; }
                continue;
            }
            result = connect => match result {
                Ok(stream) => stream,
                Err(error) => {
                    publish_unavailable(
                        &state,
                        &wake,
                        generation,
                        attempt,
                        format!("Host unavailable: {error}"),
                        reconnect_delay,
                    );
                    match wait_for_retry(reconnect_delay, &mut retry, &mut stop).await {
                        WorkerSignal::Stop => break,
                        WorkerSignal::Retry => continue,
                        WorkerSignal::Elapsed => {
                            reconnect_delay = (reconnect_delay * 2).min(MAX_RECONNECT_DELAY);
                            continue;
                        }
                    }
                }
            }
        };
        let (mut reader, mut writer) = stream.into_split();
        if let Err(error) = write_desktop_frame(&mut writer, &config.registration).await {
            publish_unavailable(
                &state,
                &wake,
                generation,
                attempt,
                format!("Host registration failed: {error}"),
                reconnect_delay,
            );
            match wait_for_retry(reconnect_delay, &mut retry, &mut stop).await {
                WorkerSignal::Stop => break,
                WorkerSignal::Retry => continue,
                WorkerSignal::Elapsed => {
                    reconnect_delay = (reconnect_delay * 2).min(MAX_RECONNECT_DELAY);
                    continue;
                }
            }
        }

        // A length-prefixed read is not cancellation safe. Keep it in its own
        // bounded lane so an outbound command cannot discard a partial frame.
        let (incoming_tx, mut incoming) = mpsc::channel(32);
        let _reader = DesktopReader(tokio::spawn(async move {
            loop {
                let frame =
                    crate::local_ipc::read_json_frame::<_, LocalHostFrame>(&mut reader).await;
                let finished = !matches!(&frame, Ok(Some(_)));
                if incoming_tx.send(frame).await.is_err() || finished {
                    break;
                }
            }
        }));
        let registration_deadline = tokio::time::Instant::now() + IO_TIMEOUT;
        let mut registered = false;
        let (disconnect_reason, retry_immediately) = loop {
            let next_write = state.lock().unwrap_or_else(|v| v.into_inner()).requests.next_write();
            if let Some(frame) = next_write {
                if let Err(error) = write_desktop_frame(&mut writer, &frame).await {
                    break (format!("Host write failed: {error}"), false);
                }
            }
            let request_deadline = state.lock().unwrap_or_else(|v| v.into_inner()).requests.deadline();
            tokio::select! {
                _ = stop.changed() => return,
                _ = tokio::time::sleep_until(registration_deadline), if !registered => {
                    break ("Local Host registration timed out after 8 seconds".to_owned(), false);
                }
                _ = tokio::time::sleep_until(request_deadline.unwrap_or(registration_deadline)), if request_deadline.is_some() => {
                    let mut state = state.lock().unwrap_or_else(|v| v.into_inner());
                    for failure in state.requests.expire(tokio::time::Instant::now()) {
                        if !state.push(DesktopClientEvent::RequestFailed { generation, failure }) {
                            state.replace_with_unavailable(generation, 1, "Request completion lane overflow".into(), INITIAL_RECONNECT_DELAY);
                        }
                    }
                    drop(state); wake();
                }
                value = retry.recv() => {
                    if value.is_none() { return; }
                    break ("Reconnect requested".to_owned(), true);
                }
                maybe_frame = outbound.recv() => {
                    let Some((frame_generation, frame)) = maybe_frame else { return; };
                    if frame_generation != generation {
                        continue;
                    }
                    if !state.lock().unwrap_or_else(|v| v.into_inner()).requests.writing(&frame) { continue; }
                    if let Err(error) = write_desktop_frame(&mut writer, &frame).await {
                        break (format!("Host write failed: {error}"), false);
                    }
                    if let LocalClientFrame::ApplicationRequest {
                        request: polar_tmux_protocol::ClientApplicationMessage::ControlCenterV1(request),
                    } = &frame {
                        tracing::info!(target:"polar_station_operation", component="Desktop", event="request_sent",
                            request_id=request.request_id.as_str(), generation);
                    }
                }
                result = incoming.recv() => {
                    match result.unwrap_or(Ok(None)) {
                        Ok(Some(LocalHostFrame::Ping { nonce })) => {
                            if let Err(error) = write_desktop_frame(
                                &mut writer,
                                &LocalClientFrame::Pong { nonce },
                            ).await {
                                break (format!("Host heartbeat reply failed: {error}"), false);
                            }
                        }
                        Ok(Some(frame)) => {
                            if let LocalHostFrame::Registered { host_control_versions, .. } = &frame {
                                if host_control_versions.and_then(|v| v.desktop) != Some(polar_tmux_protocol::DESKTOP_CONTROL_CONTRACT_VERSION) {
                                    break ("Desktop/Host IPC contract mismatch. Install the matching Polar Station desktop and managed Host artifacts together.".into(), false);
                                }
                            }
                            if frame.validate().is_err() {
                                break ("Host sent an invalid desktop frame".to_owned(), false);
                            }
                            if state.lock().unwrap_or_else(|value| value.into_inner())
                                .validate_inbound_extension(&frame).is_err() {
                                break ("Host sent an unnegotiated or oversized extension response".to_owned(), false);
                            }
                            let correlated = state.lock().unwrap_or_else(|v| v.into_inner()).requests.response(&frame);
                            match correlated {
                                Ok(Some(false)) => continue,
                                Err(reason) => break (format!("Host response identity error: {reason}"), false),
                                _ => {},
                            }
                            let accepted = state
                                .lock()
                                .unwrap_or_else(|value| value.into_inner())
                                .accept_host_frame(generation, frame);
                            if !accepted {
                                let ready = state
                                    .lock()
                                    .unwrap_or_else(|value| value.into_inner())
                                    .is_ready();
                                if !ready {
                                    break ("Host did not complete registration before sending data".to_owned(), false);
                                }
                                state
                                    .lock()
                                    .unwrap_or_else(|value| value.into_inner())
                                    .replace_with_unavailable(
                                        generation,
                                        attempt.saturating_add(1).max(1),
                                        "Desktop event queue reached its bound".to_owned(),
                                        reconnect_delay,
                                    );
                                wake();
                                break ("desktop event queue reached its bound".to_owned(), false);
                            }
                            if state
                                .lock()
                                .unwrap_or_else(|value| value.into_inner())
                                .is_ready()
                            {
                                if !registered {
                                    tracing::info!(target: "polar_station_operation", component="Desktop", event="ipc_registered", generation);
                                }
                                registered = true;
                                reconnect_delay = INITIAL_RECONNECT_DELAY;
                                attempt = 0;
                            }
                            wake();
                        }
                        Ok(None) => break ("Host disconnected".to_owned(), false),
                        Err(error) => break (format!("Host read failed: {error}"), false),
                    }
                }
            }
        };
        let next_attempt = attempt.saturating_add(1).max(1);
        let retry_after = if retry_immediately {
            Duration::ZERO
        } else {
            reconnect_delay
        };
        publish_unavailable(
            &state,
            &wake,
            generation,
            next_attempt,
            disconnect_reason,
            retry_after,
        );
        if !retry_immediately {
            match wait_for_retry(reconnect_delay, &mut retry, &mut stop).await {
                WorkerSignal::Stop => break,
                WorkerSignal::Retry => {}
                WorkerSignal::Elapsed => {
                    reconnect_delay = (reconnect_delay * 2).min(MAX_RECONNECT_DELAY);
                }
            }
        }
    }
}

struct DesktopReader(tokio::task::JoinHandle<()>);

impl Drop for DesktopReader {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn write_desktop_frame<W, T>(writer: &mut W, frame: &T) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    tokio::time::timeout(
        IO_TIMEOUT,
        crate::local_ipc::write_json_frame(writer, frame),
    )
    .await
    .context("desktop IPC write timed out")??;
    Ok(())
}

fn publish_unavailable(
    state: &Arc<Mutex<DesktopClientStateMachine>>,
    wake: &Wake,
    generation: u64,
    attempt: u32,
    message: String,
    retry_after: Duration,
) {
    let mut state = state.lock().unwrap_or_else(|value| value.into_inner());
    state.replace_with_unavailable(generation, attempt, message, retry_after);
    drop(state);
    wake();
}

enum WorkerSignal {
    Elapsed,
    Retry,
    Stop,
}

async fn wait_for_retry(
    delay: Duration,
    retry: &mut mpsc::Receiver<()>,
    stop: &mut watch::Receiver<bool>,
) -> WorkerSignal {
    tokio::select! {
        _ = tokio::time::sleep(delay) => WorkerSignal::Elapsed,
        value = retry.recv() => if value.is_some() {
            WorkerSignal::Retry
        } else {
            WorkerSignal::Stop
        },
        _ = stop.changed() => WorkerSignal::Stop,
    }
}

fn validate_socket_endpoint(socket_path: &PathBuf) -> Result<()> {
    let parent = socket_path
        .parent()
        .context("Host socket has no parent directory")?;
    let parent_metadata = fs::symlink_metadata(parent)
        .with_context(|| format!("cannot inspect Host socket directory {}", parent.display()))?;
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
        bail!("Host socket directory is not a real directory");
    }
    let effective_user = unsafe { libc::geteuid() };
    if parent_metadata.uid() != effective_user {
        bail!("Host socket directory belongs to another user");
    }
    if parent_metadata.permissions().mode() & 0o7777 != 0o700 {
        bail!("Host socket directory permissions are not 0700");
    }

    let socket_metadata = fs::symlink_metadata(socket_path)
        .with_context(|| format!("cannot inspect Host socket {}", socket_path.display()))?;
    if socket_metadata.file_type().is_symlink() || !socket_metadata.file_type().is_socket() {
        bail!("Host endpoint is not a real Unix socket");
    }
    if socket_metadata.uid() != effective_user {
        bail!("Host socket belongs to another user");
    }
    if socket_metadata.permissions().mode() & 0o7777 != 0o600 {
        bail!("Host socket permissions are not 0600");
    }
    Ok(())
}


