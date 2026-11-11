


use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    env,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use axum::{
    extract::{ws::Message, ws::WebSocket, ws::WebSocketUpgrade, State},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use polar_station_settings::SettingsRepository;
use polar_station_store_sqlite::SqliteInboxStore;
use polar_tmux_core::{TmuxClient, TmuxError, TmuxErrorKind, TmuxResult};
use polar_tmux_protocol::{
    derive_production_session_keys, production_pairing_proof_base64,
    verify_production_pairing_proof, ApplicationStateApiV1, ClientApplicationMessage,
    ClientCommand, ClientEnvelope, CompanionPairingPayloadV1, EncryptedFrameReader,
    EncryptedFrameWriter, ExecutionTarget, FrameDirection, FrontendActionName, FrontendCapability,
    FrontendCommand, FrontendCompletion, FrontendCompletionStatus, HostApplicationMessage,
    HostCapabilities, HostDescriptor, HostEnvelope, HostPlatform, HostResult, LocalClientFrame,
    LocalHostFrame, MobileCommandPolicy, MobileTmuxPane, MobileTmuxSession, MobileTmuxSnapshot,
    MobileTmuxWindow, PairingClientMessage, PairingHostMessage, PairingProofDirection,
    PairingTranscript, ProtocolError, TerminalBridgeHelloV1, TmuxTopologySnapshot, WorkspaceApiV1,
    WorkspaceCommandV1, WorkspaceRequestEnvelopeV1, WorkspaceResponseEnvelopeV1, WorkspaceResultV1,
    PROTOCOL_VERSION, SESSION_RUNTIME_API_VERSION, WORKSPACE_API_VERSION,
};
use rand_core::{OsRng, RngCore};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::sync::{mpsc, oneshot, Mutex, Semaphore};
use tracing::{info, warn, Instrument};

use crate::{
    discovery::DiscoveryAdvertisement,
    host::{
        application_state::ApplicationStatePublisher, control_center_service::ControlCenterService,
        inbox_service::InboxService, resource_service::ResourceService,
        session_fact_provider::TargetRegistrySessionFactProvider,
        session_lifecycle::SessionLifecycle, session_monitor::SessionMonitorSupervisor,
        session_registry::SessionRegistry, session_store::SessionStore,
        settings_service::SettingsService, storage::host_database,
        terminal_runtime::TerminalConsumerRegistry,
    },
    host_identity::HostIdentity,
    ssh_connection_manager::{platform_ssh_runtime_root, SshConnectionError, SshConnectionManager},
    target_registry::{RegisteredTarget, TargetRegistry},
};

#[cfg(unix)]
use crate::local_ipc;

mod session_management;

const REQUEST_CACHE_CAPACITY: usize = 256;
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
const FRONTEND_ACK_TIMEOUT: Duration = Duration::from_secs(8);
const FRONTEND_WRITE_TIMEOUT: Duration = Duration::from_secs(8);
const FRONTEND_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const FRONTEND_OUTBOUND_CAPACITY: usize = 32;
const WEBSOCKET_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

#[cfg(unix)]
struct AbortTaskOnDrop<T>(tokio::task::JoinHandle<T>);

#[cfg(unix)]
impl<T> Drop for AbortTaskOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Clone)]
struct AppState {
    dictate: Option<Arc<crate::dictate_host::DictateHost>>,
    host: HostDescriptor,
    identity: Arc<HostIdentity>,
    peer_credentials: Arc<dyn crate::host_identity::CredentialStore + Send + Sync>,
    targets: TargetRegistry,
    request_cache: Arc<Mutex<RequestCache>>,
    frontends: FrontendHub,
    session_lifecycle: Arc<SessionLifecycle>,
    control_center: Arc<ControlCenterService>,
    inbox: Arc<InboxService>,
    resources: Arc<ResourceService>,
    settings: Option<Arc<SettingsService>>,
    imessage: Option<Arc<crate::host::imessage_sender::ImessageSender>>,
    connectors: Option<crate::host::message_connector::ConnectorService>,
    #[cfg(feature = "codex-control")]
    codex: Option<Arc<crate::host::codex_control::CodexControlService>>,
    sessions: SessionRegistry,
    session_monitors: SessionMonitorSupervisor,
    application_state: ApplicationStatePublisher,
    ssh_connections: SshConnectionManager,
    connected_companions: Arc<AtomicU64>,
    companion_listener_ready: bool,
    authentication_slots: Arc<Semaphore>,
    terminal_consumers: TerminalConsumerRegistry,
    extensions: Arc<crate::local_extensions::LocalExtensionRegistry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientPolicy {
    DesktopFull,
    DesktopSanitized,
    Mobile,
    NotificationSender,
    ExecutionController,
}

#[derive(Debug, Clone)]
struct DispatchOutcome {
    result: HostResult,
    revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RequestCacheKey {
    principal: String,
    execution_target: String,
    request_id: String,
}

#[derive(Debug, Clone)]
struct CachedMutation {
    expected_revision: Option<u64>,
    command: ClientCommand,
    reference: Option<polar_station_session::SessionReference>,
    outcome: DispatchOutcome,
}

#[derive(Debug, Default)]
struct RequestCache {
    entries: HashMap<RequestCacheKey, Arc<RequestCacheEntry>>,
    order: VecDeque<RequestCacheKey>,
}

#[derive(Debug, Default)]
struct RequestCacheEntry {
    completed: Mutex<Option<CachedMutation>>,
}

impl RequestCache {
    fn entry(&mut self, key: RequestCacheKey) -> Arc<RequestCacheEntry> {
        if let Some(entry) = self.entries.get(&key) {
            return Arc::clone(entry);
        }

        let entry = Arc::new(RequestCacheEntry::default());
        self.order.push_back(key.clone());
        self.entries.insert(key, Arc::clone(&entry));
        entry
    }

    fn trim_completed(&mut self) {
        let mut remaining_candidates = self.order.len();
        while self.entries.len() > REQUEST_CACHE_CAPACITY && remaining_candidates > 0 {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            let can_evict = self.entries.get(&oldest).is_some_and(|entry| {
                Arc::strong_count(entry) == 1 && entry.completed.try_lock().is_ok()
            });
            if can_evict {
                self.entries.remove(&oldest);
            } else {
                self.order.push_back(oldest);
            }
            remaining_candidates -= 1;
        }
    }
}

#[derive(Clone)]
struct FrontendHub {
    registrations: Arc<Mutex<HashMap<String, FrontendRegistration>>>,
    pending: Arc<Mutex<HashMap<String, PendingFrontendAck>>>,
    pending_workspace_relays: Arc<Mutex<HashMap<String, PendingWorkspaceRelay>>>,
    next_id: Arc<AtomicU64>,
}

#[derive(Clone)]
struct FrontendRegistration {
    registration_id: String,
    platform: HostPlatform,
    supported_actions: BTreeSet<FrontendActionName>,
    capabilities: BTreeSet<FrontendCapability>,
    sender: mpsc::Sender<LocalHostFrame>,
}

struct PendingFrontendAck {
    registration_id: String,
    sender: oneshot::Sender<HostResult>,
}

struct PendingWorkspaceRelay {
    registration_id: String,
    request_id: String,
    scope: polar_tmux_protocol::WorkspaceScopeV1,
    sender: oneshot::Sender<WorkspaceResponseEnvelopeV1>,
}

impl FrontendHub {
    fn new() -> Self {
        Self {
            registrations: Default::default(),
            pending: Default::default(),
            pending_workspace_relays: Default::default(),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    async fn register(
        &self,
        frontend_id: String,
        platform: HostPlatform,
        supported_actions: Vec<FrontendActionName>,
        capabilities: Vec<FrontendCapability>,
        sender: mpsc::Sender<LocalHostFrame>,
    ) -> String {
        let registration_id = format!(
            "registration-{}",
            self.next_id.fetch_add(1, Ordering::Relaxed)
        );
        self.registrations.lock().await.insert(
            frontend_id,
            FrontendRegistration {
                registration_id: registration_id.clone(),
                platform,
                supported_actions: supported_actions.into_iter().collect(),
                capabilities: capabilities.into_iter().collect(),
                sender,
            },
        );
        registration_id
    }

    /// Retires this frontend lease only when it is still the active
    /// registration. The return value is also the authority to clean up
    /// frontend-scoped domain state; a replaced socket must not clean up the
    /// replacement's principal.
    async fn unregister(&self, frontend_id: &str, registration_id: &str) -> bool {
        let mut registrations = self.registrations.lock().await;
        if registrations
            .get(frontend_id)
            .is_some_and(|entry| entry.registration_id == registration_id)
        {
            registrations.remove(frontend_id);
            return true;
        }
        false
    }

    async fn dispatch(
        &self,
        platform: HostPlatform,
        execution_target: ExecutionTarget,
        revision: u64,
        action: FrontendCommand,
        attach_plan: Option<polar_tmux_protocol::TmuxAttachPlan>,
        snapshot: Option<MobileTmuxSnapshot>,
    ) -> HostResult {
        let registration = {
            let registrations = self.registrations.lock().await;
            registrations
                .values()
                .find(|registration| {
                    registration.platform == platform
                        && registration
                            .supported_actions
                            .contains(&action.action_name())
                        && (!matches!(action, FrontendCommand::Refresh)
                            || registration
                                .capabilities
                                .contains(&FrontendCapability::RefreshSnapshotV1))
                })
                .cloned()
        };
        let Some(registration) = registration else {
            return protocol_failure(
                "frontend_unavailable",
                "no registered desktop frontend supports this action",
                true,
            );
        };

        let dispatch_id = format!("dispatch-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(
            dispatch_id.clone(),
            PendingFrontendAck {
                registration_id: registration.registration_id.clone(),
                sender,
            },
        );

        let frame = LocalHostFrame::FrontendExecute {
            dispatch_id: dispatch_id.clone(),
            execution_target,
            revision,
            action,
            attach_plan,
            snapshot,
        };
        let deadline = tokio::time::Instant::now() + FRONTEND_ACK_TIMEOUT;
        match registration.sender.try_send(frame) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.pending.lock().await.remove(&dispatch_id);
                return protocol_failure(
                    "frontend_unavailable",
                    "the registered desktop frontend disconnected",
                    true,
                );
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.pending.lock().await.remove(&dispatch_id);
                return protocol_failure(
                    "frontend_backpressure",
                    "the desktop frontend queue is full",
                    true,
                );
            }
        }

        let result = match tokio::time::timeout_at(deadline, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => protocol_failure(
                "frontend_unavailable",
                "the registered desktop frontend disconnected",
                true,
            ),
            Err(_) => protocol_failure(
                "frontend_timeout",
                "the desktop frontend did not acknowledge the action",
                true,
            ),
        };
        // A successful acknowledgement removes itself in `complete`. Remove
        // again here so timeout and disconnect paths cannot retain stale
        // dispatch state.
        self.pending.lock().await.remove(&dispatch_id);
        result
    }

    async fn complete(&self, registration_id: &str, dispatch_id: &str, result: HostResult) -> bool {
        let mut pending = self.pending.lock().await;
        let matches_registration = pending
            .get(dispatch_id)
            .is_some_and(|entry| entry.registration_id == registration_id);
        if !matches_registration {
            return false;
        }
        let Some(pending) = pending.remove(dispatch_id) else {
            return false;
        };
        pending.sender.send(result).is_ok()
    }

    async fn relay_workspace(
        &self,
        platform: HostPlatform,
        request: WorkspaceRequestEnvelopeV1,
    ) -> WorkspaceResponseEnvelopeV1 {
        let registration = {
            let registrations = self.registrations.lock().await;
            registrations
                .values()
                .find(|registration| {
                    registration.platform == platform
                        && registration
                            .capabilities
                            .contains(&FrontendCapability::WorkspaceRelayV1)
                })
                .cloned()
        };
        let Some(registration) = registration else {
            return workspace_relay_error(
                &request,
                "desktop_workspace_unavailable",
                "the native desktop Workspace owner is offline",
                true,
            );
        };

        let dispatch_id = format!(
            "workspace-dispatch-{}",
            self.next_id.fetch_add(1, Ordering::Relaxed)
        );
        let (sender, receiver) = oneshot::channel();
        self.pending_workspace_relays.lock().await.insert(
            dispatch_id.clone(),
            PendingWorkspaceRelay {
                registration_id: registration.registration_id.clone(),
                request_id: request.request_id.clone(),
                scope: request.scope.clone(),
                sender,
            },
        );
        let frame = LocalHostFrame::WorkspaceRelay {
            dispatch_id: dispatch_id.clone(),
            request: request.clone(),
        };
        let deadline = tokio::time::Instant::now() + FRONTEND_ACK_TIMEOUT;
        match registration.sender.try_send(frame) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.pending_workspace_relays
                    .lock()
                    .await
                    .remove(&dispatch_id);
                return workspace_relay_error(
                    &request,
                    "desktop_workspace_unavailable",
                    "the native desktop Workspace owner disconnected",
                    true,
                );
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.pending_workspace_relays
                    .lock()
                    .await
                    .remove(&dispatch_id);
                return workspace_relay_error(
                    &request,
                    "desktop_workspace_backpressure",
                    "the native desktop Workspace owner queue is full",
                    true,
                );
            }
        }

        let response = match tokio::time::timeout_at(deadline, receiver).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => workspace_relay_error(
                &request,
                "desktop_workspace_unavailable",
                "the native desktop Workspace owner disconnected",
                true,
            ),
            Err(_) => workspace_relay_error(
                &request,
                "desktop_workspace_timeout",
                "the native desktop Workspace owner did not respond",
                true,
            ),
        };
        self.pending_workspace_relays
            .lock()
            .await
            .remove(&dispatch_id);
        response
    }

    async fn complete_workspace_relay(
        &self,
        registration_id: &str,
        dispatch_id: &str,
        response: WorkspaceResponseEnvelopeV1,
    ) -> bool {
        let mut pending = self.pending_workspace_relays.lock().await;
        let matches_request = pending.get(dispatch_id).is_some_and(|entry| {
            entry.registration_id == registration_id
                && entry.request_id == response.request_id
                && entry.scope == response.scope
        });
        if !matches_request {
            return false;
        }
        let Some(pending) = pending.remove(dispatch_id) else {
            return false;
        };
        pending.sender.send(response).is_ok()
    }
}

fn workspace_relay_error(
    request: &WorkspaceRequestEnvelopeV1,
    code: &str,
    message: &str,
    retryable: bool,
) -> WorkspaceResponseEnvelopeV1 {
    WorkspaceResponseEnvelopeV1 {
        protocol_version: PROTOCOL_VERSION,
        api: WorkspaceApiV1::WorkspaceV1,
        request_id: request.request_id.clone(),
        scope: request.scope.clone(),
        revision: 0,
        result: WorkspaceResultV1::Error {
            error: ProtocolError::new(code, message, retryable),
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostRuntimeComposition {
    StandalonePolarStation,
    EmbeddedPolar,
}

#[derive(Debug, Clone)]
pub struct HostRuntimeConfig {
    pub composition: HostRuntimeComposition,
    pub local_client: TmuxClient,
    pub companion_listen: SocketAddr,
    pub target_config: Option<PathBuf>,
    pub ipc_socket: Option<PathBuf>,
    pub runtime_data_root: Option<PathBuf>,
    pub advertise: bool,
    pub parent_lease_stdin: bool,
}

impl HostRuntimeConfig {
    pub fn validate(&self) -> Result<()> {
        if self.composition == HostRuntimeComposition::EmbeddedPolar && self.parent_lease_stdin {
            anyhow::bail!("an embedded Polar Station Host cannot lease polar's stdin");
        }
        Ok(())
    }
}

pub async fn run(
    local_client: TmuxClient,
    listen: SocketAddr,
    target_config: Option<&Path>,
    ipc_socket: Option<PathBuf>,
    runtime_data_root: Option<PathBuf>,
    advertise: bool,
    parent_lease_stdin: bool,
) -> Result<()> {
    run_config(HostRuntimeConfig {
        composition: HostRuntimeComposition::StandalonePolarStation,
        local_client,
        companion_listen: listen,
        target_config: target_config.map(Path::to_path_buf),
        ipc_socket,
        runtime_data_root,
        advertise,
        parent_lease_stdin,
    })
    .await
}

pub async fn run_config(config: HostRuntimeConfig) -> Result<()> {
    run_config_with_extensions(config, crate::local_extensions::LocalExtensionRegistry::default()).await
}

/// Explicit composition hook. The default Host owns no Polar provider or SDK.
pub async fn run_config_with_extensions(
    config: HostRuntimeConfig, extensions: crate::local_extensions::LocalExtensionRegistry,
) -> Result<()> {
    run_config_with_extensions_until(config, extensions, std::future::pending()).await
}

/// The embedding owner requests shutdown; Host still drains its own IPC,
/// terminals, resources and SSH managers before the Kernel is torn down.
pub async fn run_config_with_extensions_until<S>(
    config: HostRuntimeConfig,
    extensions: crate::local_extensions::LocalExtensionRegistry,
    shutdown: S,
) -> Result<()>
where
    S: std::future::Future<Output = ()> + Send,
{
    run_config_with_session_runtime_until(config, extensions, None, shutdown).await
}

/// Full Polar mounts the same Host-owned catalog and input operations in its SDK.
pub async fn run_config_with_session_runtime_until<S>(
    config: HostRuntimeConfig,
    extensions: crate::local_extensions::LocalExtensionRegistry,
    sessions: Option<Arc<polar_station_session::SessionRuntime>>,
    shutdown: S,
) -> Result<()>
where
    S: std::future::Future<Output = ()> + Send,
{
    run_config_with_services_until(config, extensions, HostServices { sessions, ..Default::default() }, shutdown).await
}

#[derive(Default)]
pub struct HostServices {
    pub sessions: Option<Arc<polar_station_session::SessionRuntime>>,
    pub dictate: Option<Arc<polar_dictate::runtime::DictateRuntime>>,
    pub dictate_resources: Option<crate::dictate_host::DictateResources>,
}

pub async fn run_config_with_services_until<S>(
    config: HostRuntimeConfig,
    extensions: crate::local_extensions::LocalExtensionRegistry,
    services: HostServices,
    shutdown: S,
) -> Result<()> where S: std::future::Future<Output=()> + Send {
    tokio::pin!(shutdown);
    config.validate()?;
    let session_runtime = services.sessions;
    let HostRuntimeConfig {
        composition,
        local_client,
        companion_listen: listen,
        target_config,
        ipc_socket,
        runtime_data_root,
        advertise,
        parent_lease_stdin,
    } = config;
    info!(?composition, "starting Polar Station Host runtime");
    #[cfg(unix)]
    let socket_path = match ipc_socket {
        Some(path) => path,
        None => local_ipc::default_socket_path()?,
    };
    #[cfg(unix)]
    let instance_lock = match local_ipc::acquire_host_instance_lock(&socket_path) {
        Ok(lock) => lock,
        Err(error) if parent_lease_stdin => {
            // An independent login service keeps owning the runtime. The desktop
            // helper retains only its parent's lease; closing that UI cannot stop
            // the registered Host or create a competing server/database owner.
            verify_existing_host(&socket_path)
                .await
                .with_context(|| error.to_string())?;
            return tokio::select! {
                result = async { wait_for_parent_lease_eof(&mut tokio::io::stdin()).await } =>
                    result.context("desktop parent lease failed"),
                _ = &mut shutdown => Ok(()),
            };
        }
        Err(error) => return Err(error),
    };

    #[cfg(not(unix))]
    let _ = ipc_socket;

    // IPC/data isolation alone does not isolate the SSH masters and their
    // ownership receipts. Reuse the existing private file-lock primitive so
    // another Host cannot unlink this namespace's live control sockets.
    let ssh_runtime_root = platform_ssh_runtime_root();
    #[cfg(unix)]
    let _ssh_runtime_owner =
        local_ipc::acquire_host_instance_lock(&ssh_runtime_root.join("owner")).context(
            "SSH runtime namespace already owned; isolate XDG_RUNTIME_DIR for a separate Host",
        )?;

    if advertise && listen.ip().is_loopback() {
        anyhow::bail!("mDNS advertisement requires a non-loopback Host listener");
    }
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("failed to bind Polar Tmux Host at {listen}"))?;
    let bound_address = listener
        .local_addr()
        .context("failed to read Polar Tmux Host listener address")?;

    tracing::info!(target: "polar_station_operation", component="Host", event="host_initialization_started");
    let identity = Arc::new(HostIdentity::load_platform_isolated()?);
    tracing::info!(target: "polar_station_operation", component="Host", event="host_identity_loaded");
    tracing::info!(target: "polar_station_operation", component="Host", event="host_targets_load_begin");
    let targets = TargetRegistry::load(local_client, target_config.as_deref())?;
    tracing::info!(target: "polar_station_operation", component="Host", event="host_targets_loaded");
    let database = host_database(runtime_data_root, identity.host_id())?;
    let dictate = if let Some(runtime) = services.dictate {
        Some(Arc::new(crate::dictate_host::DictateHost::embedded(runtime)))
    } else if let Some(resources) = services.dictate_resources {
        Some(Arc::new(crate::dictate_host::DictateHost::from_resources(database.path().with_file_name("dictate.db"), polar_station_settings::SettingsRepository::from_database(database.clone()), resources).await?))
    } else if composition == HostRuntimeComposition::StandalonePolarStation {
        let home = crate::host::codex_config::default_codex_config_path().map_err(anyhow::Error::msg)?.parent()
            .context("Codex home unavailable")?.to_owned();
        match crate::dictate_host::DictateHost::standalone(database.path().with_file_name("dictate.db"),polar_station_settings::SettingsRepository::from_database(database.clone()),home,
            format!("os-user:{}",crate::local_ipc::effective_uid())).await {
            Ok(service) => Some(Arc::new(service)),
            Err(_) => { tracing::error!("Voice initialization failed; other Host services remain available"); None }
        }
    } else { None };
    tracing::info!(target: "polar_station_operation", component="Host", event="host_database_opened");
    let ssh_connections = SshConnectionManager::system(ssh_runtime_root);
    ensure_configured_ssh_supervisors(&targets, &ssh_connections);
    #[cfg(unix)]
    let mut extensions = extensions;
    #[cfg(unix)]
    let polar_gateway = crate::polar_gateway::install(&mut extensions, &targets, &ssh_connections)?;
    let frontends = FrontendHub::new();
    let application_state = ApplicationStatePublisher::new();
    let sessions = SessionRegistry::with_epoch_summary_provider_and_events(
        format!("host-epoch-{}", random_uuid_v4()),
        None,
        Some(application_state.clone()),
    )
    .context("failed to initialize the Session runtime registry")?;
    tracing::info!(target: "polar_station_operation", component="Host", event="host_sessions_load_begin");
    let session_lifecycle = Arc::new(
        SessionLifecycle::open(SessionStore::from_database(database.clone()))
            .context("failed to load Polar Station Session state")?,
    );
    tracing::info!(target: "polar_station_operation", component="Host", event="host_sessions_loaded");
    let host = host_descriptor(&identity, &targets);
    let resource_targets = targets
        .descriptors()
        .into_iter()
        .map(|target| {
            let kind = if target.kind == polar_tmux_protocol::ExecutionTargetKind::Ssh {
                polar_tmux_resources::ResourceTargetKind::Ssh
            } else {
                polar_tmux_resources::ResourceTargetKind::Local
            };
            (target.id.as_str().to_owned(), kind)
        })
        .collect();
    tracing::info!(target: "polar_station_operation", component="Host", event="host_resources_open_begin");
    let resources = Arc::new(
        ResourceService::open(
            database.clone(),
            resource_targets,
            targets.clone(),
            ssh_connections.clone(),
            application_state.clone(),
        )
        .await
        .map_err(anyhow::Error::msg)?,
    );
    tracing::info!(target: "polar_station_operation", component="Host", event="host_resources_opened");
    let control_center = Arc::new(
        ControlCenterService::new_with_runtime_ports_events_and_targets(
            session_lifecycle.clone(),
            Arc::new(
                TargetRegistrySessionFactProvider::new(targets.clone(), ssh_connections.clone())
                    .with_lifecycle(session_lifecycle.clone()),
            ),
            application_state.clone(),
            targets.clone(),
            ssh_connections.clone(),
            sessions.clone(),
        ),
    );
    control_center.attach_connection_dependencies(resources.clone(), SettingsRepository::from_database(database.clone()), polar_gateway.clone());
    let session_monitors =
        SessionMonitorSupervisor::default().with_lifecycle(session_lifecycle.clone());
    tracing::info!(target: "polar_station_operation", component="Host", event="host_settings_open_begin");
    let settings = Arc::new(
        SettingsService::open(
            SettingsRepository::from_database(database.clone()),
            sessions.clone(),
            session_monitors.clone(),
            application_state.clone(),
        )
        .await
        .context("failed to load Polar Station Settings")?,
    );
    tracing::info!(target: "polar_station_operation", component="Host", event="host_settings_opened");
    let codex_ledger =
        polar_station_store_sqlite::SqliteCodexStore::from_database(database.clone())?;
    codex_ledger.recover_after_restart(crate::host::imessage_sender::now_ms())?;
    codex_ledger.recover_connectors(crate::host::imessage_sender::now_ms())?;
    tracing::info!(target: "polar_station_operation", component="Host", event="host_ledger_recovered");
    let connectors = crate::host::message_connector::ConnectorService::new(
        identity.host_id().to_owned(),
        SettingsRepository::from_database(database.clone()),
        codex_ledger.clone(),
        application_state.clone(),
    );
    #[cfg(feature = "codex-control")]
    let codex = crate::host::codex_control::CodexControlService::open(
        identity.host_id().to_owned(),
        SettingsRepository::from_database(database.clone()),
        codex_ledger.clone(),
        application_state.clone(),
        sessions.clone(),
        Some(crate::host::ssh_socket_forward::CodexNetwork {
            targets: targets.clone(),
            ssh: ssh_connections.clone(),
        }),
        Arc::new(crate::host_identity::PlatformCredentialStore),
    )
    .map_err(anyhow::Error::msg)?;
    #[cfg(feature = "codex-control")]
    {
        codex.attach_session_lifecycle(session_lifecycle.clone());
        assert!(control_center.codex.set(codex.clone()).is_ok());
    }
    #[cfg(feature = "telegram-channel")]
    let telegram = crate::host::codex_telegram_runtime::TelegramRuntime::start(
        codex.clone(),
        Arc::new(crate::host_identity::PlatformCredentialStore),
    );
    let imessage = Arc::new(
        crate::host::imessage_sender::ImessageSender::new(
            identity.host_id().to_owned(),
            SettingsRepository::from_database(database.clone()),
            codex_ledger.clone(),
            Arc::new(crate::host::imessage_sender::SystemMessagesPlatform),
            Arc::new(crate::host_identity::PlatformCredentialStore),
        )
        .with_events(application_state.clone()),
    );
    let inbox = Arc::new(
        InboxService::open(
            SqliteInboxStore::from_database(database)
                .context("failed to open Polar Station Inbox store")?,
            sessions.clone(),
            application_state.clone(),
        )
        .map_err(anyhow::Error::msg)
        .context("failed to load Polar Station Inbox")?,
    );
    #[cfg(feature = "codex-control")]
    inbox.attach_codex(codex.clone());
    #[cfg(feature = "codex-control")]
    let imessage_delivery = crate::host::codex_imessage_runtime::ImessageRuntime::start(
        codex.clone(),
        imessage.clone(),
        Arc::new(crate::host_identity::PlatformCredentialStore),
    );
    #[cfg(feature = "codex-control")]
    let connector_monitor = crate::host::connector_monitor::ConnectorMonitor::start(codex.clone());
    tracing::info!(target: "polar_station_operation", component="Host", event="host_services_ready");
    let state = Arc::new(AppState {
        dictate,
        host,
        identity,
        peer_credentials: Arc::new(crate::host_identity::PlatformCredentialStore),
        targets,
        request_cache: Default::default(),
        frontends,
        session_lifecycle,
        control_center,
        inbox,
        resources,
        settings: Some(settings),
        imessage: Some(imessage),
        connectors: Some(connectors),
        #[cfg(feature = "codex-control")]
        codex: Some(codex),
        sessions,
        session_monitors,
        application_state,
        ssh_connections,
        connected_companions: Arc::new(AtomicU64::new(0)),
        companion_listener_ready: companion_listener_ready(bound_address),
        authentication_slots: Arc::new(Semaphore::new(16)),
        terminal_consumers: TerminalConsumerRegistry::default(),
        extensions: Arc::new(extensions),
    });
    let _session_backend = session_management::mount(&state, session_runtime.as_ref())?;

    #[cfg(unix)]
    let (ipc_stop, ipc_task) = {
        let listener = local_ipc::bind(&socket_path)?;
        tracing::info!(target: "polar_station_operation", component="Host", event="ipc_listening");
        let ipc_state = state.clone();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let result = run_local_listener(listener, ipc_state, stopped).await;
            if let Err(error) = &result {
                warn!(%error, "Polar Tmux local IPC listener stopped");
            }
            result
        });
        info!(path = %socket_path.display(), "Polar Tmux local IPC listening");
        (stop, task)
    };

    #[cfg(not(unix))]
    let _ = ipc_socket;

    let resource_recovery = state.resources.start_background_recovery();
    let session_runtime = start_session_runtime(state.clone());
    let channel_retention = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            tracing::info!(target: "polar_station_operation", component="Maintenance", event="channel_retention_execute");
            let ledger = codex_ledger.clone();
            match tokio::task::spawn_blocking(move || {
                ledger.expire_transient_text(crate::host::imessage_sender::now_ms())
            })
            .await
            {
                Ok(Ok(())) => {}
                _ => warn!("channel ledger retention failed"),
            }
        }
    });

    let _advertisement = if advertise {
        Some(DiscoveryAdvertisement::advertise(
            state.identity.host_id(),
            bound_address.port(),
        )?)
    } else {
        None
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/host", get(get_host))
        .route("/v1/ws", get(websocket_upgrade))
        .with_state(state.clone());
    info!(listen = %bound_address, authenticated = true, "Polar Tmux Host listening");
    let serve = async {
        if parent_lease_stdin {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let mut stdin = tokio::io::stdin();
                    if let Err(error) = wait_for_parent_lease_eof(&mut stdin).await {
                        warn!(
                            %error,
                            "Polar Tmux Host parent lease failed"
                        );
                    }
                })
                .await
        } else {
            axum::serve(listener, app).await
        }
    };
    let serve_result = if composition == HostRuntimeComposition::StandalonePolarStation {
        tokio::select! {
            result = serve => result,
            result = wait_for_host_shutdown_signal() => result,
            _ = &mut shutdown => Ok(()),
        }
    } else {
        tokio::select! {
            result = serve => result,
            _ = &mut shutdown => Ok(()),
        }
    };
    tracing::info!(target: "polar_station_operation", component="Host", event="host_shutdown_requested");
    #[cfg(unix)]
    let ipc_result = {
        state.terminal_consumers.lifetimes.close();
        let _ = ipc_stop.send(());
        ipc_task.await.context("local IPC owner failed")
    };
    let terminal_cleanup = state
        .terminal_consumers
        .lifetimes
        .drain(Duration::from_secs(5))
        .await;
    state.resources.shutdown().await;
    if let Some(resource_recovery) = resource_recovery {
        let _ = resource_recovery.await;
    }
    session_runtime.abort();
    channel_retention.abort();
    let _ = session_runtime.await;
    let _ = channel_retention.await;
    tracing::info!(target: "polar_station_operation", component="Host", event="host_channel_workers_stopping");
    #[cfg(feature = "codex-control")]
    {
        connector_monitor.shutdown().await;
        imessage_delivery.shutdown().await;
    }
    #[cfg(feature = "telegram-channel")]
    telegram.shutdown().await;
    tracing::info!(target: "polar_station_operation", component="Host", event="host_channel_workers_stopped");
    state.session_monitors.shutdown().await;
    #[cfg(unix)]
    if let Some(gateway) = &polar_gateway { gateway.shutdown().await; }
    state.ssh_connections.shutdown().await;
    if terminal_cleanup.is_err() {
        tracing::error!(target: "polar_station_operation", component="Host", event="host_shutdown_failed", reason="terminal_cleanup_incomplete");
    }
    terminal_cleanup?;
    #[cfg(unix)]
    ipc_result??;
    tracing::info!(target: "polar_station_operation", component="Host", event="host_shutdown_complete");
    #[cfg(unix)]
    drop(instance_lock);
    serve_result?;
    Ok(())
}

fn start_session_runtime(state: Arc<AppState>) -> tokio::task::JoinHandle<()> {
    use crate::host::session_lifecycle::runtime::{self, RefreshRuntime};
    runtime::start(Arc::new(RefreshRuntime {
        targets: state.targets.clone(),
        session_lifecycle: state.session_lifecycle.clone(),
        ssh_connections: state.ssh_connections.clone(),
        sessions: state.sessions.clone(),
        session_monitors: state.session_monitors.clone(),
        control_center: state.control_center.clone(),
    }))
}

fn ensure_configured_ssh_supervisors(
    targets: &TargetRegistry,
    ssh_connections: &SshConnectionManager,
) {
    for target in targets.ssh_targets() {
        if let Err(error) = target.ensure_supervisor(ssh_connections) {
            warn!(
                execution_target_id = %target.target_id,
                %error,
                "configured SSH supervisor could not be armed at Host startup"
            );
        }
    }
}

#[cfg(unix)]
async fn verify_existing_host(socket_path: &std::path::Path) -> Result<()> {
    use crate::desktop_client::{DesktopClient, DesktopClientConfig, DesktopClientEvent};
    let wake = Arc::new(tokio::sync::Notify::new());
    let signal = wake.clone();
    let mut client = DesktopClient::new(
        DesktopClientConfig {
            socket_path: socket_path.into(),
            event_capacity: 32,
            registration: polar_tmux_protocol::LocalClientFrame::Register {
                frontend_id: format!("host-lease-{}", random_uuid_v4()),
                platform: HostPlatform::current(),
                supported_actions: vec![FrontendActionName::Refresh],
                capabilities: vec![],
                frontend_instance_id: None,
                frontend_generation: 0,
                extensions: Vec::new(),
                host_control_versions: None,
            },
        },
        move || signal.notify_one(),
    )?;
    client.start()?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            while let Some(event) = client.pop_event() {
                match event {
                    DesktopClientEvent::Ready { .. } => return Ok::<(), anyhow::Error>(()),
                    DesktopClientEvent::Unavailable { message, .. } if message.contains("contract mismatch") => anyhow::bail!(message),
                    _ => {},
                }
            }
            wake.notified().await;
        }
    })
    .await
    .context("existing Host did not complete same-user registration")??;
    Ok(())
}

async fn wait_for_host_shutdown_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            _ = terminate.recv() => Ok(()),
            result = tokio::signal::ctrl_c() => result,
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}

async fn wait_for_parent_lease_eof<R>(reader: &mut R) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; 64];
    loop {
        if reader.read(&mut buffer).await? == 0 {
            return Ok(());
        }
    }
}

async fn health() -> &'static str {
    "ok"
}

async fn get_host(State(state): State<Arc<AppState>>) -> Json<HostDescriptor> {
    Json(state.host.clone())
}

async fn websocket_upgrade(
    State(state): State<Arc<AppState>>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    upgrade
        .max_message_size(polar_tmux_protocol::MAX_APPLICATION_PAYLOAD_BYTES + 128)
        .max_frame_size(polar_tmux_protocol::MAX_APPLICATION_PAYLOAD_BYTES + 128)
        .on_upgrade(move |socket| websocket_pair_and_serve(socket, state))
}

async fn websocket_pair_and_serve(mut socket: WebSocket, state: Arc<AppState>) {
    let Ok(authentication_slot) = state.authentication_slots.clone().try_acquire_owned() else {
        send_pairing_error(
            &mut socket,
            "host_busy",
            "the Host is handling too many authentication attempts",
            true,
        )
        .await;
        return;
    };

    let pairing =
        match tokio::time::timeout(AUTH_TIMEOUT, pair_connection(&mut socket, &state)).await {
            Ok(Ok(pairing)) => pairing,
            Ok(Err(error)) => {
                warn!(%error, "Companion pairing rejected");
                send_pairing_error(
                    &mut socket,
                    "unauthorized",
                    "Companion authentication failed",
                    false,
                )
                .await;
                tokio::time::sleep(Duration::from_millis(400)).await;
                return;
            }
            Err(_) => {
                send_pairing_error(
                    &mut socket,
                    "authentication_timeout",
                    "Companion authentication timed out",
                    true,
                )
                .await;
                return;
            }
        };

    drop(authentication_slot);
    let companion = pairing.policy == ClientPolicy::Mobile;
    if companion {
        state.connected_companions.fetch_add(1, Ordering::AcqRel);
    }
    encrypted_websocket_loop(socket, state.clone(), pairing).await;
    if companion {
        state.connected_companions.fetch_sub(1, Ordering::AcqRel);
    }
}

struct AuthenticatedPairing {
    principal: String,
    policy: ClientPolicy,
    reader: EncryptedFrameReader,
    writer: EncryptedFrameWriter,
}

async fn pair_connection(socket: &mut WebSocket, state: &AppState) -> Result<AuthenticatedPairing> {
    let hello = receive_pairing_client_message(socket).await?;
    let (protocol_version, host_id, device_id, client_nonce, principal, policy, auth_key) =
        match hello {
            PairingClientMessage::ClientHello {
                protocol_version,
                host_id,
                device_id,
                client_nonce,
            } => {
                let principal = format!("mobile:{device_id}");
                (
                    protocol_version,
                    host_id,
                    device_id,
                    client_nonce,
                    principal,
                    ClientPolicy::Mobile,
                    zeroize::Zeroizing::new(*state.identity.root_secret()),
                )
            }
            PairingClientMessage::HostPeerHello {
                protocol_version,
                host_id,
                source_host_id,
                scope,
                client_nonce,
            } => {
                #[cfg(feature = "codex-control")]
                let execution_available = state.codex.is_some();
                #[cfg(not(feature = "codex-control"))]
                let execution_available = false;
                anyhow::ensure!(
                    scope == polar_tmux_protocol::HostPeerScopeV1::NotificationSender
                        || execution_available,
                    "peer capability unavailable"
                );
                let credentials = state.peer_credentials.clone();
                let source = source_host_id.clone();
                let auth_key = tokio::task::spawn_blocking(move || {
                    crate::host_identity::load_peer_key(credentials.as_ref(), &source, scope)
                })
                .await??;
                let device_id = format!("peer:{}", scope.credential_account(&source_host_id)?);
                (
                    protocol_version,
                    host_id,
                    device_id,
                    client_nonce,
                    source_host_id,
                    match scope {
                        polar_tmux_protocol::HostPeerScopeV1::NotificationSender => {
                            ClientPolicy::NotificationSender
                        }
                        polar_tmux_protocol::HostPeerScopeV1::ExecutionController => {
                            ClientPolicy::ExecutionController
                        }
                    },
                    auth_key,
                )
            }
            _ => anyhow::bail!("first pairing message must be a hello"),
        };
    hello_validation(&host_id, state.identity.host_id(), protocol_version)?;

    let transcript = PairingTranscript {
        protocol_version,
        host_id: host_id.clone(),
        device_id: device_id.clone(),
        challenge_id: random_uuid_v4(),
        client_nonce_base64: client_nonce,
        server_nonce_base64: random_nonce_base64(),
    };
    transcript
        .validate()
        .context("invalid pairing transcript")?;
    let challenge_proof = production_pairing_proof_base64(
        &auth_key,
        &transcript,
        PairingProofDirection::HostChallenge,
    )
    .context("failed to produce Host challenge proof")?;
    let expires_unix_seconds = unix_seconds() + AUTH_TIMEOUT.as_secs() as i64;
    send_pairing_host_message(
        socket,
        &PairingHostMessage::HostChallenge {
            challenge_id: transcript.challenge_id.clone(),
            host_id: host_id.clone(),
            server_nonce: transcript.server_nonce_base64.clone(),
            expires_unix_seconds,
            host_proof: challenge_proof,
        },
    )
    .await?;

    let proof_message = receive_pairing_client_message(socket).await?;
    let PairingClientMessage::ClientProof {
        challenge_id,
        proof,
    } = proof_message
    else {
        anyhow::bail!("second pairing message must be client_proof");
    };
    if challenge_id != transcript.challenge_id || unix_seconds() > expires_unix_seconds {
        anyhow::bail!("pairing challenge is stale or does not match");
    }
    let proof = STANDARD
        .decode(proof)
        .context("client proof is not base64")?;
    verify_production_pairing_proof(
        &auth_key,
        &transcript,
        PairingProofDirection::ClientProof,
        &proof,
    )
    .context("client proof is invalid")?;

    let session_id = random_uuid_v4();
    let accepted_proof = production_pairing_proof_base64(
        &auth_key,
        &transcript,
        PairingProofDirection::HostAccepted,
    )
    .context("failed to produce Host accepted proof")?;
    send_pairing_host_message(
        socket,
        &PairingHostMessage::HostAccepted {
            session_id,
            host_proof: accepted_proof,
        },
    )
    .await?;

    let keys = derive_production_session_keys(&auth_key, &transcript)
        .context("failed to derive Companion session keys")?;
    Ok(AuthenticatedPairing {
        principal,
        policy,
        reader: EncryptedFrameReader::new(*keys.client_to_host(), FrameDirection::ClientToHost),
        writer: EncryptedFrameWriter::new(*keys.host_to_client(), FrameDirection::HostToClient),
    })
}

async fn encrypted_websocket_loop(
    mut socket: WebSocket,
    state: Arc<AppState>,
    mut pairing: AuthenticatedPairing,
) {
    let mut domain_events = state.application_state.subscribe();
    let mut connector_subscription = false;
    loop {
        let received = tokio::select! {
            event = domain_events.recv(), if pairing.policy == ClientPolicy::ExecutionController || connector_subscription => {
                let Ok(event) = event else {
                    // Reconnect requires a fresh domain snapshot; do not silently
                    // continue an execution subscription after a broadcast gap.
                    return;
                };
                let permitted = match &event {
                    LocalHostFrame::ApplicationStateChanged{api:ApplicationStateApiV1::CodexV1,..}=>pairing.policy==ClientPolicy::ExecutionController,
                    LocalHostFrame::ApplicationStateChanged{api:ApplicationStateApiV1::NotificationV1,..}=>connector_subscription,
                    _=>false,
                };
                if permitted {
                    let Ok(payload) = serde_json::to_vec(&event) else { return };
                    let Ok(frame) = pairing.writer.seal(&payload) else { return };
                    if socket.send(Message::Binary(frame)).await.is_err() { return; }
                }
                continue;
            }
            received = tokio::time::timeout(WEBSOCKET_IDLE_TIMEOUT, socket.recv()) => received,
        };
        let message = match received {
            Ok(Some(Ok(message))) => message,
            Ok(Some(Err(error))) => {
                warn!(%error, "encrypted Companion receive failed");
                return;
            }
            Ok(None) | Err(_) => return,
        };
        let Message::Binary(frame) = message else {
            if matches!(message, Message::Close(_)) {
                return;
            }
            if matches!(message, Message::Ping(_) | Message::Pong(_)) {
                continue;
            }
            warn!("cleartext application frame rejected after Companion pairing");
            let _ = socket.send(Message::Close(None)).await;
            return;
        };

        let plaintext = match pairing.reader.open(&frame) {
            Ok(plaintext) => plaintext,
            Err(error) => {
                warn!(%error, "encrypted Companion frame rejected");
                return;
            }
        };
        let request: ClientApplicationMessage = match serde_json::from_slice(&plaintext) {
            Ok(request) => request,
            Err(_) => {
                // Serde errors can quote an invalid string value from a private
                // prompt or credential field. Keep application bodies out of logs.
                warn!("decrypted Companion request is invalid JSON");
                return;
            }
        };
        if (pairing.policy == ClientPolicy::NotificationSender
            && !matches!(&request, ClientApplicationMessage::NotificationV1(_)))
            || (pairing.policy == ClientPolicy::ExecutionController
                && !matches!(&request, ClientApplicationMessage::CodexV1(_)))
        {
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
        let subscribing = pairing.policy == ClientPolicy::NotificationSender
            && matches!(&request,ClientApplicationMessage::NotificationV1(r) if matches!(&r.command,polar_tmux_protocol::NotificationCommandV1::Connector{command} if matches!(command.as_ref(),polar_tmux_protocol::ConnectorCommandV1::Subscribe{..})));
        let response =
            process_application_request(&state, &pairing.principal, pairing.policy, request).await;
        if subscribing
            && matches!(&response,HostApplicationMessage::NotificationV1(r) if matches!(&r.result,polar_tmux_protocol::NotificationResultV1::Connector{result} if matches!(result.as_ref(),polar_tmux_protocol::ConnectorResultV1::Inbound{..})))
        {
            connector_subscription = true;
        }
        let payload = match serde_json::to_vec(&response) {
            Ok(payload) => payload,
            Err(error) => {
                warn!(%error, "failed to encode Companion response");
                return;
            }
        };
        let frame = match pairing.writer.seal(&payload) {
            Ok(frame) => frame,
            Err(error) => {
                warn!(%error, "failed to encrypt Companion response");
                return;
            }
        };
        if socket.send(Message::Binary(frame)).await.is_err() {
            return;
        }
    }
}

async fn receive_pairing_client_message(socket: &mut WebSocket) -> Result<PairingClientMessage> {
    let message = socket
        .recv()
        .await
        .context("Companion closed before pairing completed")?
        .context("failed to receive Companion pairing message")?;
    let Message::Text(text) = message else {
        anyhow::bail!("pairing messages must be text JSON");
    };
    let message: PairingClientMessage =
        serde_json::from_str(&text).context("pairing message is invalid JSON")?;
    message.validate().context("pairing message is invalid")?;
    Ok(message)
}

async fn send_pairing_host_message(
    socket: &mut WebSocket,
    message: &PairingHostMessage,
) -> Result<()> {
    message
        .validate()
        .context("Host pairing message is invalid")?;
    let json = serde_json::to_string(message).context("failed to encode pairing message")?;
    socket
        .send(Message::Text(json))
        .await
        .context("failed to send pairing message")
}

async fn send_pairing_error(socket: &mut WebSocket, code: &str, message: &str, retryable: bool) {
    let _ = send_pairing_host_message(
        socket,
        &PairingHostMessage::Error {
            error: ProtocolError::new(code, message, retryable),
        },
    )
    .await;
}

fn hello_validation(host_id: &str, expected_host_id: &str, protocol_version: u32) -> Result<()> {
    if protocol_version != PROTOCOL_VERSION {
        anyhow::bail!("protocol version mismatch");
    }
    if host_id != expected_host_id {
        anyhow::bail!("Host identity mismatch");
    }
    Ok(())
}

#[tracing::instrument(target="polar_station_operation", name="host_tmux_request", skip_all,
    fields(component="Host", request_id=request.request_id.as_str()))]
async fn process_request(
    state: &AppState,
    principal: &str,
    policy: ClientPolicy,
    request: ClientEnvelope,
) -> HostEnvelope {
    let request_id = request.request_id.clone();
    let execution_target = request.execution_target.clone();
    let target = state.targets.get(execution_target.as_str());
    let revision = target.as_ref().map_or(0, RegisteredTarget::revision);

    let outcome = if let Err(error) = request.validate() {
        DispatchOutcome {
            result: protocol_failure(
                "invalid_request",
                &format!("the request is invalid: {error}"),
                false,
            ),
            revision,
        }
    } else if policy == ClientPolicy::Mobile
        && MobileCommandPolicy::validate(&request.command).is_err()
    {
        DispatchOutcome {
            result: protocol_failure(
                "command_not_allowed",
                "the command is not available to Companion clients",
                false,
            ),
            revision,
        }
    } else if policy == ClientPolicy::DesktopSanitized
        && !sanitized_desktop_command_allowed(&request.command)
    {
        DispatchOutcome {
            result: protocol_failure(
                "command_not_allowed",
                "the Linux frontend may only inspect sanitized state, request typed attach, or dispatch typed frontend actions",
                false,
            ),
            revision,
        }
    } else if let Some(target) = target {
        if request.command.is_mutation() {
            dispatch_cached_mutation(state, principal, target, policy, request, None).await
        } else {
            dispatch_observed_read(state, target, policy, request.command).await
        }
    } else {
        DispatchOutcome {
            result: protocol_failure(
                "execution_target_not_found",
                "the selected execution target is not configured",
                false,
            ),
            revision,
        }
    };

    HostEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id,
        execution_target,
        revision: outcome.revision,
        result: outcome.result,
    }
}

async fn process_application_request(
    state: &AppState,
    principal: &str,
    policy: ClientPolicy,
    request: ClientApplicationMessage,
) -> HostApplicationMessage {
    match request {
        ClientApplicationMessage::CodexV1(request) => {
            let unavailable = polar_tmux_protocol::CodexResponseEnvelopeV1 {
                protocol_version: PROTOCOL_VERSION,
                api: polar_tmux_protocol::CodexApiV1::CodexV1,
                request_id: request.request_id.clone(),
                owner_host_id: state.identity.host_id().to_owned(),
                result: polar_tmux_protocol::CodexResultV1::Error {
                    code: "codex_control_unavailable".into(),
                },
            };
            #[cfg(feature = "codex-control")]
            let response = match state.codex.as_ref().filter(|_| {
                !matches!(
                    policy,
                    ClientPolicy::Mobile | ClientPolicy::NotificationSender
                )
            }) {
                Some(service) => {
                    service
                        .dispatch(
                            principal,
                            policy != ClientPolicy::ExecutionController,
                            *request,
                        )
                        .await
                }
                None => unavailable,
            };
            #[cfg(not(feature = "codex-control"))]
            let response = unavailable;
            HostApplicationMessage::CodexV1(Box::new(response))
        }
        ClientApplicationMessage::NotificationV1(request) => {
            if matches!(
                request.command,
                polar_tmux_protocol::NotificationCommandV1::Connector { .. }
            ) && !matches!(
                policy,
                ClientPolicy::Mobile | ClientPolicy::ExecutionController
            ) {
                if let Some(service) = &state.connectors {
                    return HostApplicationMessage::NotificationV1(Box::new(service.dispatch(
                        principal,
                        policy != ClientPolicy::NotificationSender,
                        *request,
                    )));
                }
            }
            let response = match state.imessage.as_ref().filter(|_| {
                !matches!(
                    policy,
                    ClientPolicy::Mobile | ClientPolicy::ExecutionController
                )
            }) {
                None => polar_tmux_protocol::NotificationResponseEnvelopeV1 {
                    protocol_version: PROTOCOL_VERSION,
                    api: request.api,
                    request_id: request.request_id,
                    result: polar_tmux_protocol::NotificationResultV1::Error {
                        code: if matches!(
                            policy,
                            ClientPolicy::Mobile | ClientPolicy::ExecutionController
                        ) {
                            "scope_denied"
                        } else {
                            "sender_unavailable"
                        }
                        .into(),
                    },
                },
                Some(sender) => {
                    sender
                        .dispatch(
                            principal,
                            policy != ClientPolicy::NotificationSender,
                            *request,
                        )
                        .await
                }
            };
            HostApplicationMessage::NotificationV1(Box::new(response))
        }
        ClientApplicationMessage::ControlCenterV1(request) => {
            let reason = match &request.command {
                polar_tmux_protocol::ControlCenterCommandV1::GetProjection => "get_projection",
                polar_tmux_protocol::ControlCenterCommandV1::SessionEditor { .. } => "session_editor",
                polar_tmux_protocol::ControlCenterCommandV1::SubmitIntent { .. } => "submit_intent",
            };
            let span = tracing::info_span!(target:"polar_station_operation", "host_control_request",
                component="Host", request_id=request.request_id.as_str(), reason, outcome="cancelled");
            let response = state
                .control_center
                .dispatch(principal, request)
                .instrument(span.clone())
                .await;
            span.record(
                "outcome",
                if matches!(
                    &response.result,
                    polar_tmux_protocol::ControlCenterResultV1::Error { .. }
                ) {
                    "failed"
                } else {
                    "ok"
                },
            );
            HostApplicationMessage::ControlCenterV1(Box::new(response))
        }
        ClientApplicationMessage::InboxV1(request) => {
            if policy == ClientPolicy::Mobile {
                HostApplicationMessage::InboxV1(Box::new(
                    polar_tmux_protocol::InboxResponseEnvelopeV1 {
                        protocol_version: PROTOCOL_VERSION,
                        api: request.api,
                        request_id: request.request_id,
                        event_revision: 0,
                        result: polar_tmux_protocol::InboxResultV1::Error {
                            error: ProtocolError::new(
                                "command_not_allowed",
                                "Companion clients cannot read or edit the desktop Inbox",
                                false,
                            ),
                            message: None,
                        },
                    },
                ))
            } else {
                HostApplicationMessage::InboxV1(Box::new(state.inbox.dispatch(*request).await))
            }
        }
        ClientApplicationMessage::ResourcesV1(request) => {
            if policy == ClientPolicy::Mobile {
                let revision = state.resources.current_revision().await;
                HostApplicationMessage::ResourcesV1(Box::new(
                    polar_tmux_protocol::ResourcesResponseEnvelopeV1 {
                        protocol_version: PROTOCOL_VERSION,
                        api: request.api,
                        request_id: request.request_id,
                        revision,
                        result: polar_tmux_protocol::ResourcesResultV1::Error {
                            error: ProtocolError::new(
                                "command_not_allowed",
                                "Companion clients cannot read or edit the desktop Resource catalog",
                                false,
                            ),
                            projection: None,
                        },
                    },
                ))
            } else {
                #[cfg(feature = "codex-control")]
                let response=state.resources.dispatch_from_session(*request,state.codex.as_deref(),Some(crate::host::resource_service::SessionFileContext {
                    lifecycle: &state.session_lifecycle,
                    targets: &state.targets,
                    ssh: &state.ssh_connections,
                })).await;
                #[cfg(not(feature = "codex-control"))]
                let response=state.resources.dispatch(*request).await;
                HostApplicationMessage::ResourcesV1(Box::new(response))
            }
        }
        ClientApplicationMessage::SettingsV1(request) => {
            if policy == ClientPolicy::Mobile {
                HostApplicationMessage::SettingsV1(Box::new(
                    polar_tmux_protocol::SettingsResponseEnvelopeV1 {
                        protocol_version: PROTOCOL_VERSION,
                        api: request.api,
                        request_id: request.request_id,
                        revision: 0,
                        result: polar_tmux_protocol::SettingsResultV1::Error {
                            error: ProtocolError::new(
                                "command_not_allowed",
                                "Companion clients cannot read or edit desktop Settings",
                                false,
                            ),
                            projection: None,
                        },
                    },
                ))
            } else {
                match &state.settings {
                    Some(settings) => HostApplicationMessage::SettingsV1(Box::new(
                        settings.dispatch(*request).await,
                    )),
                    None => HostApplicationMessage::SettingsV1(Box::new(
                        polar_tmux_protocol::SettingsResponseEnvelopeV1 {
                            protocol_version: PROTOCOL_VERSION,
                            api: request.api,
                            request_id: request.request_id,
                            revision: 0,
                            result: polar_tmux_protocol::SettingsResultV1::Error {
                                error: ProtocolError::new(
                                    "settings_unavailable",
                                    "Settings are unavailable",
                                    true,
                                ),
                                projection: None,
                            },
                        },
                    )),
                }
            }
        }
        ClientApplicationMessage::TmuxV1(request) => {
            HostApplicationMessage::TmuxV1(process_request(state, principal, policy, request).await)
        }
        ClientApplicationMessage::WorkspaceV1(request) => {
            if policy == ClientPolicy::Mobile {
                if !mobile_workspace_command_allowed(&request.command) {
                    return HostApplicationMessage::WorkspaceV1(workspace_relay_error(
                        &request,
                        "command_not_allowed",
                        "the Workspace command is not available to Companion clients",
                        false,
                    ));
                }
                return HostApplicationMessage::WorkspaceV1(
                    state
                        .frontends
                        .relay_workspace(state.host.platform, request)
                        .await,
                );
            }
            // Workspace messages are relayed to the native owner, never exported
            // from a Host mirror or used as Session recovery input.
            HostApplicationMessage::WorkspaceV1(
                state
                    .frontends
                    .relay_workspace(state.host.platform, request)
                    .await,
            )
        }
    }
}

fn mobile_workspace_command_allowed(command: &WorkspaceCommandV1) -> bool {
    matches!(
        command,
        WorkspaceCommandV1::GetCatalog
            | WorkspaceCommandV1::GetWorkspace
            | WorkspaceCommandV1::SelectWorkspace { .. }
            | WorkspaceCommandV1::SelectTab { .. }
            | WorkspaceCommandV1::SetDisplayMode { .. }
            | WorkspaceCommandV1::SelectFocusPane { .. }
            | WorkspaceCommandV1::Open
            | WorkspaceCommandV1::Repair
            | WorkspaceCommandV1::FocusPrevious
            | WorkspaceCommandV1::FocusNext
            | WorkspaceCommandV1::FocusSidebarSlot { .. }
    )
}

fn sanitized_desktop_command_allowed(command: &ClientCommand) -> bool {
    matches!(
        command,
        ClientCommand::Ping
            | ClientCommand::GetHost
            | ClientCommand::ListExecutionTargets
            | ClientCommand::GetSnapshot
            | ClientCommand::ListSessions
            | ClientCommand::ListWindows { .. }
            | ClientCommand::ListPanes { .. }
            | ClientCommand::PrepareAttach { .. }
            | ClientCommand::AwaitAttachRecovery { .. }
            | ClientCommand::Frontend {
                action: FrontendCommand::Activate
                    | FrontendCommand::OpenSession { .. }
                    | FrontendCommand::FocusPane { .. }
                    | FrontendCommand::Refresh,
            }
    )
}

async fn dispatch_cached_mutation(
    state: &AppState,
    principal: &str,
    target: RegisteredTarget,
    policy: ClientPolicy,
    request: ClientEnvelope,
    reference: Option<polar_station_session::SessionReference>,
) -> DispatchOutcome {
    let key = RequestCacheKey {
        principal: principal.to_string(),
        execution_target: request.execution_target.to_string(),
        request_id: request.request_id.clone(),
    };
    let entry = state.request_cache.lock().await.entry(key);
    let mut completed = entry.completed.lock().await;
    if let Some(cached) = completed.as_ref() {
        return if cached.command == request.command
            && cached.expected_revision == request.expected_revision
            && cached.reference == reference
        {
            cached.outcome.clone()
        } else {
            DispatchOutcome {
                result: protocol_failure(
                    "request_id_conflict",
                    "this principal already used the request ID for another mutation",
                    false,
                ),
                revision: target.revision(),
            }
        };
    }

    let operation_gate = target.operation_gate();
    let operation_guard = operation_gate.write().await;
    let (revision, observed_topology) = match refresh_target_revision(state, &target).await {
        Ok(observation) => observation,
        Err(error) => {
            let outcome = DispatchOutcome {
                result: HostResult::Error { error },
                revision: target.revision(),
            };
            *completed = Some(CachedMutation {
                expected_revision: request.expected_revision,
                command: request.command.clone(),
                reference: reference.clone(),
                outcome: outcome.clone(),
            });
            drop(completed);
            drop(entry);
            drop(operation_guard);
            state.request_cache.lock().await.trim_completed();
            return outcome;
        }
    };
    if request
        .expected_revision
        .is_some_and(|expected| expected != revision)
    {
        let outcome = DispatchOutcome {
            result: protocol_failure(
                "stale_snapshot",
                "the selected execution target changed; refresh before retrying",
                true,
            ),
            revision,
        };
        *completed = Some(CachedMutation {
            expected_revision: request.expected_revision,
            command: request.command.clone(),
            reference: reference.clone(),
            outcome: outcome.clone(),
        });
        drop(completed);
        drop(entry);
        drop(operation_guard);
        state.request_cache.lock().await.trim_completed();
        return outcome;
    }

    let command = request.command.clone();
    let refresh_snapshot = matches!(
        command,
        ClientCommand::Frontend {
            action: FrontendCommand::Refresh
        }
    )
    .then(|| MobileTmuxSnapshot::from(&observed_topology));
    let revision_before_dispatch = target.revision();
    let mut outcome = if let Some(reference) = reference.as_ref() {
        session_management::guarded_input(state, &target, command.clone(), reference).await
    } else {
        dispatch_command(
            state,
            target.clone(),
            policy,
            command.clone(),
            refresh_snapshot,
        )
        .await
    };
    if target.revision() == revision_before_dispatch
        && outcome_may_have_mutated(&command, &outcome.result)
    {
        let topology = match target.managed_client(&state.ssh_connections).await {
            Ok(client) => client.topology_snapshot().await.ok(),
            Err(_) => None,
        };
        target.record_mutation(topology);
    }
    outcome.revision = target.revision();
    *completed = Some(CachedMutation {
        expected_revision: request.expected_revision,
        command: request.command,
        reference,
        outcome: outcome.clone(),
    });
    drop(completed);
    drop(entry);
    drop(operation_guard);
    state.request_cache.lock().await.trim_completed();
    outcome
}

fn outcome_may_have_mutated(command: &ClientCommand, result: &HostResult) -> bool {
    if matches!(
        command,
        ClientCommand::Frontend {
            action: FrontendCommand::Refresh
        }
    ) {
        return false;
    }
    if command.is_tmux_mutation() {
        return true;
    }
    match result {
        HostResult::Error { error } => matches!(
            error.code.as_str(),
            "tmux_applied_snapshot_unavailable" | "frontend_applied_snapshot_unavailable"
        ),
        HostResult::FrontendCompleted { completion, .. } => {
            completion.status != FrontendCompletionStatus::FrontendFailed
        }
        _ => true,
    }
}

async fn dispatch_observed_read(
    state: &AppState,
    target: RegisteredTarget,
    policy: ClientPolicy,
    command: ClientCommand,
) -> DispatchOutcome {
    if matches!(command, ClientCommand::AwaitAttachRecovery { .. }) {
        return dispatch_command(state, target, policy, command, None).await;
    }
    let operation_gate = target.operation_gate();
    let _operation_guard = operation_gate.read().await;
    if command_observes_target(&command) {
        if let Err(error) = refresh_target_revision(state, &target).await {
            return DispatchOutcome {
                result: HostResult::Error { error },
                revision: target.revision(),
            };
        }
    }
    let mut outcome = dispatch_command(state, target.clone(), policy, command, None).await;
    outcome.revision = target.revision();
    outcome
}

async fn refresh_target_revision(
    state: &AppState,
    target: &RegisteredTarget,
) -> Result<(u64, TmuxTopologySnapshot), ProtocolError> {
    let client = target
        .managed_client(&state.ssh_connections)
        .await
        .map_err(ssh_protocol_error)?;
    let topology = client.topology_snapshot().await.map_err(protocol_error)?;
    let revision = target.record_topology_observation(topology.clone());
    Ok((revision, topology))
}

fn command_observes_target(command: &ClientCommand) -> bool {
    matches!(
        command,
        ClientCommand::GetSnapshot
            | ClientCommand::ListSessions
            | ClientCommand::ListWindows { .. }
            | ClientCommand::ListPanes { .. }
            | ClientCommand::ListClients
            | ClientCommand::CapturePane { .. }
            | ClientCommand::PrepareAttach { .. }
    )
}

async fn dispatch_command(
    state: &AppState,
    target: RegisteredTarget,
    policy: ClientPolicy,
    command: ClientCommand,
    refresh_snapshot: Option<MobileTmuxSnapshot>,
) -> DispatchOutcome {
    if let ClientCommand::Frontend { action } = &command {
        return dispatch_frontend_command(state, target, action.clone(), refresh_snapshot).await;
    }
    debug_assert!(refresh_snapshot.is_none());

    let system_result = match &command {
        ClientCommand::Ping => Some(HostResult::Pong),
        ClientCommand::GetHost => Some(HostResult::Host {
            host: state.host.clone(),
        }),
        ClientCommand::GetCompanionPairing if policy == ClientPolicy::DesktopFull => {
            let pairing_code = state.identity.pairing_code();
            Some(HostResult::CompanionPairing {
                pairing: CompanionPairingPayloadV1::new(
                    state.identity.host_id(),
                    pairing_code.as_str(),
                    state.connected_companions.load(Ordering::Acquire) as u32,
                    state.companion_listener_ready,
                )
                .expect("Host identity always produces a valid pairing payload"),
            })
        }
        ClientCommand::GetCompanionPairing => Some(protocol_failure(
            "command_not_allowed",
            "the Companion pairing credential is available only to the local Mac frontend",
            false,
        )),
        ClientCommand::ListExecutionTargets => Some(HostResult::ExecutionTargets {
            targets: state.targets.descriptors(),
        }),
        _ => None,
    };
    if let Some(result) = system_result {
        return DispatchOutcome {
            result,
            revision: target.revision(),
        };
    }
    if let ClientCommand::AwaitAttachRecovery {
        session,
        after_transport_generation,
    } = &command
    {
        let Some(destination) = target.ssh_destination() else {
            return DispatchOutcome {
                result: HostResult::Error {
                    error: ProtocolError::new(
                        "ssh_recovery_unavailable",
                        "Only an SSH target can await transport recovery.",
                        false,
                    ),
                },
                revision: target.revision(),
            };
        };
        let generation = match target
            .wait_for_connected_after(
                &state.ssh_connections,
                destination,
                Some(after_transport_generation.clone()),
            )
            .await
        {
            Ok(generation) => generation,
            Err(error) => {
                return DispatchOutcome {
                    result: HostResult::Error {
                        error: ssh_protocol_error(error),
                    },
                    revision: target.revision(),
                };
            }
        };
        let attach = match target
            .managed_client_for_attach(&state.ssh_connections)
            .await
        {
            Ok(client) => client,
            Err(error) => {
                return DispatchOutcome {
                    result: HostResult::Error {
                        error: ssh_protocol_error(error),
                    },
                    revision: target.revision(),
                };
            }
        };
        let runtime_session = match state
            .session_lifecycle
            .resolve(&attach.client, target.id(), session)
            .await
        {
            Ok(session) => session,
            Err(error) => {
                return DispatchOutcome {
                    result: protocol_failure("session_unavailable", &error.to_string(), true),
                    revision: target.revision(),
                }
            }
        };
        let plan = match attach.client.prepare_attach_known_session(&runtime_session) {
            Ok(plan) => plan,
            Err(error) => {
                return DispatchOutcome {
                    result: HostResult::Error {
                        error: protocol_error(error),
                    },
                    revision: target.revision(),
                };
            }
        };
        return DispatchOutcome {
            result: HostResult::AttachPlan {
                plan,
                transport_generation: attach.transport_generation.or(Some(generation)),
            },
            revision: target.revision(),
        };
    }
    if let ClientCommand::PrepareAttach { session } = &command {
        let attach = match target
            .managed_client_for_attach(&state.ssh_connections)
            .await
        {
            Ok(client) => client,
            Err(error) => {
                return DispatchOutcome {
                    result: HostResult::Error {
                        error: ssh_protocol_error(error),
                    },
                    revision: target.revision(),
                };
            }
        };
        let runtime_session = match state
            .session_lifecycle
            .resolve(&attach.client, target.id(), session)
            .await
        {
            Ok(session) => session,
            Err(error) => {
                return DispatchOutcome {
                    result: protocol_failure("session_unavailable", &error.to_string(), true),
                    revision: target.revision(),
                }
            }
        };
        let plan = match attach.client.prepare_attach_known_session(&runtime_session) {
            Ok(plan) => plan,
            Err(error) => {
                return DispatchOutcome {
                    result: HostResult::Error {
                        error: protocol_error(error),
                    },
                    revision: target.revision(),
                };
            }
        };
        return DispatchOutcome {
            result: HostResult::AttachPlan {
                plan,
                transport_generation: attach.transport_generation,
            },
            revision: target.revision(),
        };
    }

    let client = match target.managed_client(&state.ssh_connections).await {
        Ok(client) => client,
        Err(error) => {
            return DispatchOutcome {
                result: HostResult::Error {
                    error: ssh_protocol_error(error),
                },
                revision: target.revision(),
            };
        }
    };

    if let ClientCommand::CreateSession {
        name,
        working_directory,
        command: process,
    } = &command
    {
        let result: Result<HostResult> = async {
            let mut launch =
                polar_tmux_workspace::SessionLaunchSpec::shell(name, working_directory.as_deref());
            if let Some(command) = process {
                launch.process = polar_tmux_workspace::ProcessRestoreSpec::Command {
                    command: command.clone(),
                    auto_restart: false,
                };
            }
            let record = state
                .session_lifecycle
                .create(
                    &client,
                    polar_tmux_workspace::ExecutionTargetId::new(target.id())?,
                    launch,
                )
                .await?;
            let session = client
                .list_sessions()
                .await?
                .into_iter()
                .find(|s| s.id == record.runtime_tmux_session_id.as_str())
                .context("created Session disappeared")?;
            Ok(match policy {
                ClientPolicy::Mobile => HostResult::MobileSessionCreated {
                    session: MobileTmuxSession::from(&session),
                },
                ClientPolicy::DesktopSanitized => HostResult::SanitizedSessionCreated {
                    session: MobileTmuxSession::from(&session),
                },
                _ => HostResult::SessionCreated { session },
            })
        }
        .await;
        return DispatchOutcome {
            result: result.unwrap_or_else(|error| {
                error.downcast_ref::<TmuxError>().map_or_else(
                    || {
                        protocol_failure(
                            "session_create_failed",
                            "The tmux Session could not be created.",
                            false,
                        )
                    },
                    |error| HostResult::Error {
                        error: protocol_error(error.clone()),
                    },
                )
            }),
            revision: target.revision(),
        };
    }
    if let ClientCommand::KillSession { target: reference } = &command {
        let result: Result<()> = async {
            let target_id = polar_tmux_workspace::ExecutionTargetId::new(target.id())?;
            let reference = state
                .session_lifecycle
                .managed_reference(&client, target.id(), reference)
                .await?;
            state
                .session_lifecycle
                .end_session(&client, &target_id, &reference)
                .await
        }
        .await;
        return DispatchOutcome {
            result: result
                .map(|_| HostResult::Accepted)
                .unwrap_or_else(|error| {
                    error.downcast_ref::<TmuxError>().map_or_else(
                        || {
                            protocol_failure(
                                "session_end_failed",
                                "The tmux Session could not be ended.",
                                false,
                            )
                        },
                        |error| HostResult::Error {
                            error: protocol_error(error.clone()),
                        },
                    )
                }),
            revision: target.revision(),
        };
    }
    let registry_command = command.clone();
    let result: TmuxResult<HostResult> = async {
        Ok(match command {
            ClientCommand::Ping => HostResult::Pong,
            ClientCommand::GetHost => HostResult::Host {
                host: state.host.clone(),
            },
            ClientCommand::GetCompanionPairing => {
                unreachable!("handled as a local-only system result")
            }
            ClientCommand::ListExecutionTargets => HostResult::ExecutionTargets {
                targets: state.targets.descriptors(),
            },
            ClientCommand::GetSnapshot => {
                if policy == ClientPolicy::DesktopFull {
                    HostResult::Snapshot {
                        snapshot: client.snapshot().await?,
                    }
                } else {
                    let snapshot = client.topology_snapshot().await?;
                    if policy == ClientPolicy::Mobile {
                        HostResult::MobileSnapshot {
                            snapshot: MobileTmuxSnapshot::from(&snapshot),
                        }
                    } else {
                        HostResult::SanitizedSnapshot {
                            snapshot: MobileTmuxSnapshot::from(&snapshot),
                        }
                    }
                }
            }
            ClientCommand::ListSessions => {
                let sessions = client.list_sessions().await?;
                if policy == ClientPolicy::Mobile {
                    HostResult::MobileSessions {
                        sessions: sessions.iter().map(MobileTmuxSession::from).collect(),
                    }
                } else if policy == ClientPolicy::DesktopSanitized {
                    HostResult::SanitizedSessions {
                        sessions: sessions.iter().map(MobileTmuxSession::from).collect(),
                    }
                } else {
                    HostResult::Sessions { sessions }
                }
            }
            ClientCommand::ListWindows { session } => {
                let windows = client.list_windows(&session).await?;
                if policy == ClientPolicy::Mobile {
                    HostResult::MobileWindows {
                        windows: windows.iter().map(MobileTmuxWindow::from).collect(),
                    }
                } else if policy == ClientPolicy::DesktopSanitized {
                    HostResult::SanitizedWindows {
                        windows: windows.iter().map(MobileTmuxWindow::from).collect(),
                    }
                } else {
                    HostResult::Windows { windows }
                }
            }
            ClientCommand::ListPanes { target } => {
                if policy == ClientPolicy::DesktopFull {
                    HostResult::Panes {
                        panes: client.list_panes(&target).await?,
                    }
                } else if policy == ClientPolicy::Mobile {
                    let panes = client.list_topology_panes(&target).await?;
                    HostResult::MobilePanes {
                        panes: panes.iter().map(MobileTmuxPane::from).collect(),
                    }
                } else {
                    let panes = client.list_topology_panes(&target).await?;
                    HostResult::SanitizedPanes {
                        panes: panes.iter().map(MobileTmuxPane::from).collect(),
                    }
                }
            }
            ClientCommand::ListClients => HostResult::Clients {
                clients: client.list_clients().await?,
            },
            ClientCommand::CreateSession { .. } | ClientCommand::KillSession { .. } => {
                unreachable!("Session owner handled mutation")
            }
            ClientCommand::CreateWindow {
                session,
                name,
                working_directory,
                command,
            } => HostResult::WindowCreated {
                window: client
                    .create_window(
                        &session,
                        name.as_deref(),
                        working_directory.as_deref(),
                        command.as_deref(),
                    )
                    .await?,
            },
            ClientCommand::KillWindow { target } => {
                client.kill_window(&target).await?;
                HostResult::Accepted
            }
            ClientCommand::RenameWindow { target, name } => HostResult::WindowRenamed {
                window: client.rename_window(&target, &name).await?,
            },
            ClientCommand::SplitPane {
                target,
                horizontal,
                working_directory,
                command,
            } => HostResult::PaneCreated {
                pane: client
                    .split_pane(
                        &target,
                        horizontal,
                        working_directory.as_deref(),
                        command.as_deref(),
                    )
                    .await?,
            },
            ClientCommand::KillPane { target } => {
                client.kill_pane(&target).await?;
                HostResult::Accepted
            }
            ClientCommand::SelectPane { target } => {
                client.select_pane(&target).await?;
                HostResult::Accepted
            }
            ClientCommand::ResizePane {
                target,
                direction,
                amount,
            } => {
                client.resize_pane(&target, direction, amount).await?;
                HostResult::Accepted
            }
            ClientCommand::SendKeys { target, keys } => {
                client.send_keys(&target, &keys).await?;
                HostResult::Accepted
            }
            ClientCommand::SendText {
                target,
                text,
                press_enter,
            } => {
                client.send_text(&target, &text, press_enter).await?;
                HostResult::Accepted
            }
            ClientCommand::SwitchClient {
                client: name,
                session,
            } => {
                client.switch_client(&name, &session).await?;
                HostResult::Accepted
            }
            ClientCommand::CapturePane { .. } => {
                return Ok(protocol_failure(
                    "terminal_capture_unavailable",
                    "terminal capture is available only through the local operator CLI",
                    false,
                ));
            }
            ClientCommand::PrepareAttach { .. } => {
                unreachable!("handled before the managed client is acquired")
            }
            ClientCommand::AwaitAttachRecovery { .. } => {
                unreachable!("handled before the managed client is acquired")
            }
            ClientCommand::Frontend { .. } => unreachable!("handled above"),
        })
    }
    .await;

    if let Ok(host_result) = &result {
        sync_session_registry_after_command(state, &target, &registry_command, host_result).await;
    }

    match result {
        Ok(result) => DispatchOutcome {
            result,
            revision: target.revision(),
        },
        Err(error) => DispatchOutcome {
            result: HostResult::Error {
                error: protocol_error(error),
            },
            revision: target.revision(),
        },
    }
}

async fn sync_session_registry_after_command(
    state: &AppState,
    target: &RegisteredTarget,
    command: &ClientCommand,
    _result: &HostResult,
) {
    if command.is_tmux_mutation() {
        if let Ok(target) = polar_tmux_workspace::ExecutionTargetId::new(target.id()) {
            state.session_lifecycle.request_refresh(target);
        }
    }
}

async fn dispatch_frontend_command(
    state: &AppState,
    target: RegisteredTarget,
    action: FrontendCommand,
    refresh_snapshot: Option<MobileTmuxSnapshot>,
) -> DispatchOutcome {
    let is_refresh = matches!(action, FrontendCommand::Refresh);
    if is_refresh != refresh_snapshot.is_some() {
        return DispatchOutcome {
            result: protocol_failure(
                "frontend_snapshot_unavailable",
                "the Host could not supply the observed snapshot for this frontend action",
                true,
            ),
            revision: target.revision(),
        };
    }
    let client = match target.managed_client(&state.ssh_connections).await {
        Ok(client) => client,
        Err(error) => {
            return DispatchOutcome {
                result: HostResult::Error {
                    error: ssh_protocol_error(error),
                },
                revision: target.revision(),
            };
        }
    };
    let attach_plan = if let FrontendCommand::OpenSession { session } = &action {
        let runtime_session = match state
            .session_lifecycle
            .resolve(&client, target.id(), session)
            .await
        {
            Ok(session) => session,
            Err(error) => {
                return DispatchOutcome {
                    result: protocol_failure("session_unavailable", &error.to_string(), true),
                    revision: target.revision(),
                }
            }
        };
        match client.prepare_attach(&runtime_session).await {
            Ok(plan) => Some(plan),
            Err(error) => {
                return DispatchOutcome {
                    result: HostResult::Error {
                        error: protocol_error(error),
                    },
                    revision: target.revision(),
                };
            }
        }
    } else {
        None
    };

    let mut focus_snapshot = None;
    let revision = if let FrontendCommand::FocusPane { pane } = &action {
        if let Err(error) = client.select_pane(pane).await {
            return DispatchOutcome {
                result: HostResult::Error {
                    error: protocol_error(error),
                },
                revision: target.revision(),
            };
        }
        let topology = client.topology_snapshot().await.ok();
        focus_snapshot.clone_from(&topology);
        target.record_mutation(topology)
    } else {
        target.revision()
    };

    let tmux_was_applied = matches!(action, FrontendCommand::FocusPane { .. });
    let frontend_result = state
        .frontends
        .dispatch(
            state.host.platform,
            ExecutionTarget::new(target.id().to_string()).expect("registered target IDs are valid"),
            revision,
            action,
            attach_plan,
            refresh_snapshot.clone(),
        )
        .await;
    let frontend_was_accepted = matches!(frontend_result, HostResult::Accepted);
    let (status, frontend_error) = frontend_completion(tmux_was_applied, frontend_result);
    let snapshot = match (focus_snapshot, refresh_snapshot) {
        (Some(snapshot), None) => MobileTmuxSnapshot::from(&snapshot),
        (None, Some(snapshot)) => snapshot,
        (None, None) => match client.topology_snapshot().await {
            Ok(snapshot) => {
                if tmux_was_applied {
                    target.align_published_mutation_topology(snapshot.clone());
                }
                MobileTmuxSnapshot::from(&snapshot)
            }
            Err(_) => {
                let code = if tmux_was_applied {
                    "tmux_applied_snapshot_unavailable"
                } else if frontend_was_accepted {
                    "frontend_applied_snapshot_unavailable"
                } else {
                    "frontend_snapshot_unavailable"
                };
                return DispatchOutcome {
                    result: protocol_failure(
                        code,
                        "the frontend action completed but its resulting tmux snapshot is unavailable",
                        true,
                    ),
                    revision,
                };
            }
        },
        (Some(_), Some(_)) => unreachable!("focus and refresh snapshots are exclusive"),
    };
    DispatchOutcome {
        result: HostResult::FrontendCompleted {
            completion: FrontendCompletion {
                status,
                frontend_error,
            },
            snapshot,
        },
        revision,
    }
}

fn frontend_completion(
    tmux_was_applied: bool,
    frontend_result: HostResult,
) -> (FrontendCompletionStatus, Option<ProtocolError>) {
    match frontend_result {
        HostResult::Accepted => (FrontendCompletionStatus::Applied, None),
        HostResult::Error { .. } => (
            if tmux_was_applied {
                FrontendCompletionStatus::TmuxAppliedFrontendFailed
            } else {
                FrontendCompletionStatus::FrontendFailed
            },
            Some(ProtocolError::new(
                "frontend_failed",
                "the desktop frontend could not complete the requested action",
                true,
            )),
        ),
        _ => (
            if tmux_was_applied {
                FrontendCompletionStatus::TmuxAppliedFrontendFailed
            } else {
                FrontendCompletionStatus::FrontendFailed
            },
            Some(ProtocolError::new(
                "frontend_invalid_ack",
                "the desktop frontend returned an invalid acknowledgement",
                false,
            )),
        ),
    }
}

fn protocol_error(error: TmuxError) -> ProtocolError {
    let (code, message, retryable) = match error.kind() {
        TmuxErrorKind::InvalidInput => (
            "tmux_invalid_input",
            "the tmux operation contains invalid input",
            false,
        ),
        TmuxErrorKind::ExecutableMissing => (
            "tmux_executable_missing",
            "the configured tmux executable is unavailable",
            false,
        ),
        TmuxErrorKind::NoServer => ("tmux_no_server", "the tmux server is not running", true),
        TmuxErrorKind::TargetNotFound => (
            "tmux_target_not_found",
            "the requested tmux target was not found",
            false,
        ),
        TmuxErrorKind::UnsupportedTarget => (
            "tmux_unsupported_target",
            "the tmux operation is not available for this target",
            false,
        ),
        TmuxErrorKind::TargetUnavailable => (
            "tmux_target_unavailable",
            "the configured tmux target is unavailable",
            true,
        ),
        TmuxErrorKind::SshMasterUnavailable => (
            "ssh_master_unavailable",
            "the shared SSH master is unavailable; retry the operation",
            true,
        ),
        TmuxErrorKind::CommandFailed => ("tmux_command_failed", "the tmux operation failed", false),
        TmuxErrorKind::SessionAlreadyExists => (
            "tmux_session_already_exists",
            "a tmux session with this name already exists",
            false,
        ),
        TmuxErrorKind::ParseFailed => (
            "tmux_parse_failed",
            "the tmux response could not be parsed",
            false,
        ),
        TmuxErrorKind::Timeout => ("tmux_timeout", "the tmux operation timed out", true),
    };
    ProtocolError::new(code, message, retryable)
}

fn ssh_protocol_error(error: SshConnectionError) -> ProtocolError {
    ProtocolError::new(
        error.code().as_str(),
        error.sanitized_message(),
        error.code().retryable(),
    )
}

fn protocol_failure(code: &str, message: &str, retryable: bool) -> HostResult {
    HostResult::Error {
        error: ProtocolError::new(code, message, retryable),
    }
}

fn host_descriptor(identity: &HostIdentity, targets: &TargetRegistry) -> HostDescriptor {
    let host_name = env::var("HOSTNAME")
        .or_else(|_| env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "Polar Tmux Host".to_string());
    HostDescriptor {
        protocol_version: PROTOCOL_VERSION,
        host_id: identity.host_id().to_string(),
        host_name,
        platform: HostPlatform::current(),
        host_version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: HostCapabilities {
            tmux_read: true,
            tmux_write: true,
            frontend_control: true,
            remote_tmux: targets.has_remote_target(),
            wsl_tmux: false,
            workspace_api_versions: vec![WORKSPACE_API_VERSION],
            resources_api_versions: vec![polar_tmux_protocol::RESOURCES_API_VERSION],
            presentation_api_versions: Vec::new(),
            session_runtime_api_versions: vec![SESSION_RUNTIME_API_VERSION],
            ssh_multiplex_required: targets.has_remote_target(),
        },
    }
}

#[cfg(unix)]
async fn run_local_listener(
    listener: tokio::net::UnixListener,
    state: Arc<AppState>,
    mut stopped: tokio::sync::oneshot::Receiver<()>,
) -> Result<()> {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        let (stream, _) = tokio::select! {
            _ = &mut stopped => {
                let start = std::time::Instant::now();
                tracing::info!(target: "polar_station_operation", component="Host", event="ipc_connections_stopping", count=connections.len());
                connections.shutdown().await;
                tracing::info!(target: "polar_station_operation", component="Host", event="ipc_connections_stopped", live=connections.len(), wait_us=start.elapsed().as_micros() as u64);
                return Ok(());
            },
            _ = connections.join_next(), if !connections.is_empty() => continue,
            result = listener.accept() => result.context("failed to accept local IPC connection")?,
        };
        let connection_state = state.clone();
        connections.spawn(async move {
            if let Err(error) = serve_local_connection(stream, connection_state).await {
                warn!(error = %format!("{error:#}"), "local frontend connection closed");
            }
        });
    }
}

#[cfg(unix)]
async fn serve_local_connection(
    mut stream: tokio::net::UnixStream,
    state: Arc<AppState>,
) -> Result<()> {
    let peer_uid = stream.peer_cred().context("read local frontend peer credentials")?.uid();
    let first = tokio::time::timeout(
        AUTH_TIMEOUT,
        local_ipc::read_json_frame::<_, LocalClientFrame>(&mut stream),
    )
    .await
    .context("local frontend registration timed out")??
    .context("local frontend disconnected before registration")?;
    first
        .validate()
        .context("invalid local frontend registration")?;
    // Only this bounded one-request profile accepts an OS-root supervisor.
    // Root does not gain a desktop/terminal registration in another user's Host.
    if let LocalClientFrame::ExtensionRequest { request } = &first {
        if request.extension == crate::dictate_host::EXTENSION {
            local_ipc::verify_peer(&stream)?;
            if request.payload.get("op").and_then(serde_json::Value::as_str) == Some("subscribe_history") {
                let service=state.dictate.as_ref().context("Voice could not initialize; open Settings → Voice diagnostics")?;
                return service.serve_history(stream, request.clone()).await;
            }
            if request.payload.get("op").and_then(serde_json::Value::as_str) == Some("subscribe") {
                let service=state.dictate.as_ref().context("Voice could not initialize; open Settings → Voice diagnostics")?;
                return service.serve_preview(stream, request.clone()).await;
            }
            use polar_local_access_contract::{LocalExtensionResponseV1,LocalExtensionResultV1};
            let result = async {
                anyhow::ensure!(request.version == 1,"Update Polar Station Host to use Voice");
                let service=state.dictate.as_ref().context("Voice could not initialize; open Settings → Voice diagnostics")?;
                service.request(request.payload.clone()).await
            }.await;
            let result=match result {
                Ok(payload)=>LocalExtensionResultV1::Success {payload},
                Err(error)=>LocalExtensionResultV1::Rejected {error:polar_tmux_protocol::ProtocolError::new(
                    "dictate_request_failed",error.to_string().chars().take(400).collect::<String>(),false)},
            };
            let response=LocalExtensionResponseV1::for_request(request,result);
            response.validate()?;
            local_ipc::write_json_frame(&mut stream,&LocalHostFrame::ExtensionResponse {response}).await?;
            return Ok(());
        }
        if request.extension == crate::session_client::EXTENSION {
            anyhow::ensure!(peer_uid == local_ipc::effective_uid() || peer_uid == 0,"session control peer UID is not authorized");
            let response = session_management::local_request(&state,peer_uid,request.clone()).await;
            local_ipc::write_json_frame(&mut stream,&LocalHostFrame::ExtensionResponse { response }).await?;
            return Ok(());
        }
    }
    local_ipc::verify_peer(&stream)?;
    if let LocalClientFrame::TerminalBridgeV2 { hello } = &first {
        return serve_terminal_bridge_versioned(stream, state, hello.base.clone(), true).await;
    }
    if let LocalClientFrame::TerminalBridge { hello } = &first {
        return serve_terminal_bridge(stream, state, hello.clone()).await;
    }
    let LocalClientFrame::Register {
        host_control_versions,
        extensions: offered_extensions,
        frontend_id,
        platform,
        supported_actions,
        capabilities,
        frontend_instance_id: _,
        frontend_generation: _,
    } = first
    else {
        anyhow::bail!("first local IPC frame must register the frontend");
    };
    if platform != state.host.platform {
        anyhow::bail!("local frontend platform does not match the Host platform");
    }
    let supports_application_state_events =
        capabilities.contains(&FrontendCapability::ApplicationStateEventsV1);
    let supports_session_feed = capabilities.contains(&FrontendCapability::SessionFeedV1);
    let supports_workspace_relay = capabilities.contains(&FrontendCapability::WorkspaceRelayV1);
    let negotiated_controls = {
        let v = host_control_versions.unwrap_or_default();
        let mut supported = v.negotiated();
        supported.desktop = Some(polar_tmux_protocol::DESKTOP_CONTROL_CONTRACT_VERSION);
        #[cfg(feature = "codex-control")]
        if state.codex.is_none() {
            supported.codex = None;
        }
        #[cfg(not(feature = "codex-control"))]
        {
            supported.codex = None;
        }
        supported
    };

    let negotiated_extensions = state.extensions.negotiate(&offered_extensions)?;
    let (sender, mut receiver) = mpsc::channel(FRONTEND_OUTBOUND_CAPACITY);
    let registration_id = state
        .frontends
        .register(
            frontend_id.clone(),
            platform,
            supported_actions,
            capabilities,
            sender.clone(),
        )
        .await;
    let connection_result: Result<()> = async {
        sender
            .try_send(LocalHostFrame::Registered {
                host_control_versions: Some(negotiated_controls),
                extensions: negotiated_extensions.clone(),
                registration_id: registration_id.clone(),
                protocol_version: PROTOCOL_VERSION,
                host_id: state.identity.host_id().to_string(),
            })
            .context("failed to queue local registration acknowledgement")?;
        let (mut reader, mut writer) = stream.into_split();
        let (extension_event_sender, mut extension_event_receiver) = tokio::sync::mpsc::channel(32);
        let mut writer_task = AbortTaskOnDrop(tokio::spawn(async move {
            loop {
                let frame = tokio::select! {
                    biased;
                    frame = receiver.recv() => match frame { Some(frame) => frame, None => break },
                    Some(event) = extension_event_receiver.recv() => LocalHostFrame::ExtensionEvent { event },
                };
                frame
                    .validate()
                    .context("Host produced invalid local IPC frame")?;
                write_local_frontend_frame(
                    &mut writer,
                    &frame,
                    FRONTEND_WRITE_TIMEOUT,
                )
                .await?;
                if matches!(frame, LocalHostFrame::Registered { .. }) {
                    tracing::info!(target: "polar_station_operation", component="Host", event="frontend_registered_sent");
                }
            }
            Ok::<(), anyhow::Error>(())
        }));
        let mut heartbeat = tokio::time::interval_at(
            tokio::time::Instant::now() + FRONTEND_HEARTBEAT_INTERVAL,
            FRONTEND_HEARTBEAT_INTERVAL,
        );
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut expected_pong = None;
        let mut application_state_events = state.application_state.subscribe();
        let mut session_feed_task: Option<AbortTaskOnDrop<()>> = None;
        let mut extensions = crate::local_extensions::LocalExtensionSession::new(
            state.extensions.clone(), negotiated_extensions.clone(),
            frontend_id.clone(), registration_id.clone(), Some(peer_uid),
        ).with_event_sender(extension_event_sender);
        let extension_lifetime = extensions.lifetime();
        let mut reading = Box::pin(local_ipc::read_json_frame::<_, LocalClientFrame>(&mut reader));
        loop {
            tokio::select! {
                _ = extension_lifetime.cancelled() => {
                    anyhow::bail!("extension event stream closed; a fresh snapshot is required");
                }
                completion = extensions.next_response(), if !extensions.is_empty() => {
                    if let Some(response) = completion? {
                        if state.frontends.registrations.lock().await.get(&frontend_id)
                            .is_none_or(|entry| entry.registration_id != registration_id) {
                            anyhow::bail!("extension completion belongs to a retired frontend registration");
                        }
                        sender.try_send(LocalHostFrame::ExtensionResponse { response })
                            .context("extension response queue is full or closed")?;
                    }
                }
                event = application_state_events.recv(), if supports_application_state_events => {
                    match event {
                        Ok(frame) => {
                            if let LocalHostFrame::ApplicationStateChanged {api,..} = &frame {
                                if !negotiated_controls.permits(*api) {continue;}
                            }
                            frame.validate().context("Host produced invalid application-state frame")?;
                            sender.try_send(frame)
                                .context("frontend outbound queue is full or closed")?;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            break;
                        }
                    }
                }
                result = &mut writer_task.0 => {
                    result.context("local frontend writer task failed")??;
                    break;
                }
                frame = &mut reading => {
                    drop(reading);
                    reading = Box::pin(local_ipc::read_json_frame::<_, LocalClientFrame>(&mut reader));
                    let Some(frame) = frame? else {
                        break;
                    };
                    frame.validate().context("frontend sent invalid local IPC frame")?;
                    match frame {
                        LocalClientFrame::Register { .. } => {
                            anyhow::bail!("frontend attempted to register twice");
                        }
                        LocalClientFrame::Request { request } => {
                            let request_state = state.clone();
                            let response_sender = sender.clone();
                            let principal = format!("frontend:{frontend_id}");
                            tokio::spawn(async move {
                                let response = process_request(
                                    &request_state,
                                    &principal,
                                    if platform == HostPlatform::Linux {
                                        ClientPolicy::DesktopSanitized
                                    } else {
                                        ClientPolicy::DesktopFull
                                    },
                                    request,
                                ).await;
                                let _ = response_sender.try_send(
                                    LocalHostFrame::Response { response }
                                );
                            });
                        }
                        LocalClientFrame::ApplicationRequest { request } => {
                            if (matches!(&request,ClientApplicationMessage::NotificationV1(_)) && !negotiated_controls.permits(ApplicationStateApiV1::NotificationV1))
                                || (matches!(&request,ClientApplicationMessage::CodexV1(_)) && !negotiated_controls.permits(ApplicationStateApiV1::CodexV1)) {
                                anyhow::bail!("notification API was not negotiated");
                            }
                            let request_state = state.clone();
                            let response_sender = sender.clone();
                            let principal = format!("frontend:{frontend_id}");
                            tokio::spawn(async move {
                                let response = process_application_request(
                                    &request_state,
                                    &principal,
                                    if platform == HostPlatform::Linux {
                                        ClientPolicy::DesktopSanitized
                                    } else {
                                        ClientPolicy::DesktopFull
                                    },
                                    request,
                                ).await;
                                let _ = response_sender.try_send(
                                    LocalHostFrame::ApplicationResponse { response }
                                );
                            });
                        }
                        LocalClientFrame::ExtensionRequest { request } => {
                            if state.frontends.registrations.lock().await.get(&frontend_id)
                                .is_none_or(|entry| entry.registration_id != registration_id) {
                                anyhow::bail!("extension request belongs to a retired frontend registration");
                            }
                            if let Some(response) = extensions.submit(request)? {
                                sender.try_send(LocalHostFrame::ExtensionResponse { response })
                                    .context("extension response queue is full or closed")?;
                            }
                        }
                        LocalClientFrame::ExtensionCancel { request } => {
                            let response = extensions.cancel(request)?;
                            sender.try_send(LocalHostFrame::ExtensionResponse { response })
                                .context("extension cancellation response queue is full or closed")?;
                        }
                        LocalClientFrame::SubscribeSessionFeed {
                            known_epoch,
                            after_revision,
                        } => {
                            if !supports_session_feed {
                                anyhow::bail!("frontend subscribed without negotiating Session feed v1");
                            }
                            if let Some(task) = session_feed_task.take() {
                                drop(task);
                            }
                            session_feed_task = Some(AbortTaskOnDrop(
                                spawn_session_feed_forwarder(
                                state.sessions.clone(),
                                sender.clone(),
                                known_epoch,
                                after_revision,
                                )
                            ));
                        }
                        LocalClientFrame::TerminalBridge { .. } | LocalClientFrame::TerminalBridgeV2 { .. } => {
                            anyhow::bail!("terminal bridge upgrade must be the first local IPC frame");
                        }
                        LocalClientFrame::FrontendAck { dispatch_id, result } => {
                            if !state.frontends.complete(&registration_id, &dispatch_id, result).await {
                                anyhow::bail!("frontend acknowledgement is late, duplicate, or mismatched");
                            }
                        }
                        LocalClientFrame::WorkspaceRelayCompleted {
                            dispatch_id,
                            response,
                        } => {
                            if !supports_workspace_relay {
                                anyhow::bail!("frontend completed a Workspace relay without negotiating the capability");
                            }
                            if !state
                                .frontends
                                .complete_workspace_relay(
                                    &registration_id,
                                    &dispatch_id,
                                    response,
                                )
                                .await
                            {
                                anyhow::bail!("Workspace relay completion is late, duplicate, or mismatched");
                            }
                        }
                        LocalClientFrame::Pong { nonce } => {
                            if expected_pong != Some(nonce) {
                                anyhow::bail!("frontend heartbeat nonce does not match");
                            }
                            expected_pong = None;
                        }
                    }
                }
                _ = heartbeat.tick() => {
                    if expected_pong.is_some() {
                        anyhow::bail!("frontend heartbeat timed out");
                    }
                    let nonce = state.frontends.next_id.fetch_add(1, Ordering::Relaxed);
                    expected_pong = Some(nonce);
                    sender.try_send(LocalHostFrame::Ping { nonce })
                        .context("failed to queue frontend heartbeat")?;
                }
            }
        }
        drop(writer_task);
        drop(session_feed_task);
        Ok(())
    }
    .await;
    let owns_principal_cleanup = state
        .frontends
        .unregister(&frontend_id, &registration_id)
        .await;
    if owns_principal_cleanup {
        state
            .control_center
            .remove_principal(&format!("frontend:{frontend_id}"))
            .await;
    }
    match connection_result {
        Err(error) if is_clean_local_frontend_disconnect(&error) => Ok(()),
        result => result,
    }
}

#[cfg(unix)]
async fn write_local_frontend_frame<W>(
    writer: &mut W,
    frame: &LocalHostFrame,
    timeout: Duration,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    tokio::time::timeout(timeout, local_ipc::write_json_frame(writer, frame))
        .await
        .context("local frontend frame write timed out")??;
    Ok(())
}

#[cfg(unix)]
async fn serve_terminal_bridge(
    stream: tokio::net::UnixStream,
    state: Arc<AppState>,
    hello: TerminalBridgeHelloV1,
) -> Result<()> {
    serve_terminal_bridge_versioned(stream, state, hello, false).await
}

async fn serve_terminal_bridge_versioned(
    stream: tokio::net::UnixStream,
    state: Arc<AppState>,
    hello: TerminalBridgeHelloV1,
    control_v2: bool,
) -> Result<()> {
    crate::host::terminal_runtime::serve_bridge(
        stream,
        crate::host::terminal_runtime::TerminalServices {
            targets: state.targets.clone(),
            ssh_connections: state.ssh_connections.clone(),
            session_lifecycle: state.session_lifecycle.clone(),
            sessions: state.sessions.clone(),
            terminal_consumers: state.terminal_consumers.clone(),
        },
        hello,
        control_v2,
    )
    .await
}



#[cfg(unix)]
fn spawn_session_feed_forwarder(
    registry: SessionRegistry,
    sender: mpsc::Sender<LocalHostFrame>,
    known_epoch: Option<String>,
    after_revision: Option<u64>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut events = registry.subscribe();
        let snapshot = registry.snapshot().await;
        let cursor_is_current = known_epoch.as_deref() == Some(snapshot.epoch.as_str())
            && after_revision == Some(snapshot.revision);
        if !cursor_is_current
            && sender
                .send(LocalHostFrame::SessionFeedSnapshot { snapshot })
                .await
                .is_err()
        {
            return;
        }
        loop {
            let frame = match events.recv().await {
                Ok(event) => LocalHostFrame::SessionFeedEvent { event },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    LocalHostFrame::SessionFeedSnapshot {
                        snapshot: registry.snapshot().await,
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            };
            if sender.send(frame).await.is_err() {
                return;
            }
        }
    })
}

#[cfg(unix)]
fn is_clean_local_frontend_disconnect(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_error| {
                matches!(
                    io_error.kind(),
                    std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::NotConnected
                )
            })
    })
}

fn random_nonce_base64() -> String {
    let mut nonce = [0_u8; 32];
    OsRng.fill_bytes(&mut nonce);
    STANDARD.encode(nonce)
}

fn random_uuid_v4() -> String {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    )
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn companion_listener_ready(bound_address: SocketAddr) -> bool {
    !bound_address.ip().is_loopback()
}






