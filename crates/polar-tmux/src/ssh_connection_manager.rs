mod route;
use route::*;
pub(crate) use route::{DohProvider, SshRouteKind, SshRouteProfile};
mod diagnostics;
use diagnostics::{
    append_recovery_trace, flush_recovery_trace, record_recovery_trace, record_startup_failure,
};
mod process;
use crate::local_ipc::socket_readiness;
use process::{wait_for_master_exit, OpenSshBackend};

use std::{
    collections::{HashMap, VecDeque},
    ffi::OsString,
    fmt,
    fs::{self, OpenOptions},
    future::Future,
    io::{self, Write},
    net::Ipv4Addr,
    path::{Path, PathBuf},
    pin::Pin,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex as StdMutex, Weak,
    },
    time::{Duration, Instant as StdInstant},
};

use futures_util::FutureExt;
use polar_tmux_core::TmuxTarget;
use polar_tmux_protocol::SshTransportGenerationV1;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    process::Command,
    sync::{broadcast, oneshot, watch, Mutex},
    task::JoinHandle,
    time::Instant,
};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};

const RECEIPT_NAME: &str = "ownership.json";
const RECOVERY_TRACE_NAME: &str = "recovery-trace.v1.json";
const RECOVERY_TRACE_CAPACITY: usize = 100;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(35);
const CHECK_TIMEOUT: Duration = Duration::from_secs(2);
const LAN_CANDIDATE_TIMEOUT: Duration = Duration::from_millis(750);
const MAX_CONTROL_SOCKET_PATH_LEN: usize = 103;
const OPENSSH_TEMP_SUFFIX_BUDGET: usize = 24;
const STABLE_CONNECTION_RESET: Duration = Duration::from_secs(30);
const ROUTE_AUDIT_INTERVAL: Duration = Duration::from_secs(5 * 60);
const LAN_UPGRADE_HOLDOFF: Duration = Duration::from_secs(60);
const OLD_ROUTE_RETIRE_GRACE: Duration = Duration::from_millis(1_250);

#[cfg(not(test))]
const SUPERVISOR_REARM_DELAY: Duration = Duration::from_secs(1);
const RETRY_DELAYS: [Duration; 6] = [
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(5),
];

type CheckFuture<'a> = Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'a>>;
type ResolveFuture<'a> = Pin<Box<dyn Future<Output = io::Result<ResolvedWanRoute>> + Send + 'a>>;

trait OwnedMaster: Send {
    fn pid(&self) -> u32;
    fn is_alive(&mut self) -> io::Result<bool>;
    fn exit_notification(&self) -> watch::Receiver<bool>;
    fn exit_report(&self) -> Option<process::MasterExitReport> {
        None
    }
}

trait SshMasterBackend: Send + Sync {
    fn spawn_master(&self, spec: &MasterSpec) -> io::Result<Box<dyn OwnedMaster>>;
    fn check_master<'a>(&'a self, spec: &'a MasterSpec) -> CheckFuture<'a>;
}

#[derive(Debug, Clone)]
struct MasterSpec {
    target_id: String,
    destination: String,
    destination_fingerprint: String,
    generation: u64,
    control_socket: PathBuf,
    route_profile: Option<SshRouteProfile>,
    route: SshRoute,
}

impl MasterSpec {
    fn master_args(&self) -> Vec<OsString> {
        let (connect_timeout, keepalive) = match self.route.kind() {
            SshRouteKind::Lan => (3, 1),
            SshRouteKind::Wan => (10, 5),
            SshRouteKind::Opaque => (30, 3),
        };
        let mut args = vec![
            "-M".into(),
            "-N".into(),
            "-S".into(),
            self.control_socket.as_os_str().to_owned(),
            "-o".into(),
            "ControlMaster=yes".into(),
            "-o".into(),
            "ControlPersist=no".into(),
            "-o".into(),
            format!("ConnectTimeout={connect_timeout}").into(),
            "-o".into(),
            format!("ServerAliveInterval={keepalive}").into(),
            "-o".into(),
            "ServerAliveCountMax=2".into(),
            "-o".into(),
            "BatchMode=yes".into(),
        ];
        if let Some((address, port)) = self.route.proxy_endpoint() {
            args.extend([
                OsString::from("-o"),
                OsString::from("ProxyCommand=none"),
                OsString::from("-o"),
                OsString::from("ProxyJump=none"),
                OsString::from("-o"),
                OsString::from(format!("HostName={address}")),
                OsString::from("-p"),
                OsString::from(port.to_string()),
            ]);
            if let Some(alias) = self.route.host_key_alias() {
                args.extend([
                    OsString::from("-o"),
                    OsString::from(format!("HostKeyAlias={alias}")),
                    OsString::from("-o"),
                    OsString::from("StrictHostKeyChecking=yes"),
                    OsString::from("-o"),
                    OsString::from("CheckHostIP=no"),
                ]);
            }
        }
        args.push(self.destination.clone().into());
        args
    }

    
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SshConnectionErrorCode {
    InvalidTarget,
    OwnershipConflict,
    MasterUnavailable,
    GenerationStale,
}

impl SshConnectionErrorCode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::InvalidTarget => "ssh_target_invalid",
            Self::OwnershipConflict => "ssh_runtime_ownership_conflict",
            Self::MasterUnavailable => "ssh_master_unavailable",
            Self::GenerationStale => "ssh_master_generation_stale",
        }
    }

    pub(crate) fn retryable(self) -> bool {
        matches!(self, Self::MasterUnavailable | Self::GenerationStale)
    }
}

fn supervisor_error_is_fail_closed(code: SshConnectionErrorCode) -> bool {
    matches!(
        code,
        SshConnectionErrorCode::InvalidTarget | SshConnectionErrorCode::OwnershipConflict
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SshConnectionError {
    code: SshConnectionErrorCode,
    message: &'static str,
}

impl SshConnectionError {
    fn new(code: SshConnectionErrorCode, message: &'static str) -> Self {
        Self { code, message }
    }

    pub(crate) fn code(&self) -> SshConnectionErrorCode {
        self.code
    }

    pub(crate) fn sanitized_message(&self) -> &'static str {
        self.message
    }
}

impl fmt::Display for SshConnectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for SshConnectionError {}

#[derive(Debug, Clone)]
pub(crate) struct SshConnectionLease {
    target_id: String,
    generation: u64,
    master_pid: u32,
    destination: String,
    destination_fingerprint: String,
    control_socket: PathBuf,
    route: SshRouteKind,
    operation_id: u64,
    exited: watch::Receiver<bool>,
}

impl PartialEq for SshConnectionLease {
    fn eq(&self, other: &Self) -> bool {
        self.target_id == other.target_id
            && self.generation == other.generation
            && self.master_pid == other.master_pid
            && self.destination == other.destination
            && self.destination_fingerprint == other.destination_fingerprint
            && self.control_socket == other.control_socket
            && self.route == other.route
            && self.operation_id == other.operation_id
    }
}
impl Eq for SshConnectionLease {}

impl SshConnectionLease {
    

    pub(crate) fn master_exited(&self) -> bool {
        *self.exited.borrow()
    }

    pub(crate) async fn wait_for_exit(&self) -> bool {
        let mut exited = self.exited.clone();
        loop {
            if *exited.borrow_and_update() {
                return true;
            }
            if exited.changed().await.is_err() {
                return false;
            }
        }
    }

    pub(crate) fn target_id(&self) -> &str {
        &self.target_id
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn route(&self) -> SshRouteKind {
        self.route
    }

    

    pub(crate) fn tmux_target(&self) -> Result<TmuxTarget, SshConnectionError> {
        TmuxTarget::multiplexed_ssh(
            self.destination.clone(),
            self.control_socket.to_string_lossy().into_owned(),
        )
        .map_err(|_| {
            SshConnectionError::new(
                SshConnectionErrorCode::InvalidTarget,
                "the verified SSH target cannot be represented safely",
            )
        })
    }

    pub(crate) fn forward_control_args(&self, cancel: bool, rule: &str) -> Vec<OsString> {
        vec![
            "-S".into(),
            self.control_socket.as_os_str().to_owned(),
            "-o".into(),
            "ControlMaster=no".into(),
            "-o".into(),
            "ProxyCommand=/usr/bin/false".into(),
            "-O".into(),
            if cancel { "cancel" } else { "forward" }.into(),
            "-L".into(),
            rule.into(),
            self.destination.clone().into(),
        ]
    }
}

#[derive(Clone)]
pub(crate) struct SshConnectionManager {
    inner: Arc<ManagerInner>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SshConnectionStatus {
    Connecting,
    Connected,
    Recovering,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SshConnectionStatusChange {
    pub(crate) target_id: String,
    pub(crate) status: SshConnectionStatus,
    pub(crate) generation: Option<u64>,
    pub(crate) route: Option<SshRouteKind>,
    pub(crate) preserves_consumer_identity: bool,
}

static NEXT_MANAGER_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);

struct ManagerInner {
    instance_id: u64,
    runtime_root: PathBuf,
    backend: Arc<dyn SshMasterBackend>,
    route_backend: Arc<dyn NetworkRouteBackend>,
    entries: StdMutex<HashMap<String, Arc<Mutex<TargetEntry>>>>,
    observed_statuses: StdMutex<HashMap<String, SshConnectionObservation>>,
    verified_leases: StdMutex<HashMap<String, SshConnectionLease>>,
    status_events: broadcast::Sender<SshConnectionStatusChange>,
    network_events: broadcast::Sender<()>,
    wan_cache: StdMutex<HashMap<String, CachedWanRoute>>,
    next_operation_id: AtomicU64,
    next_supervisor_id: AtomicU64,
    supervisors: StdMutex<HashMap<String, SupervisorRecord>>,
    stopping: AtomicBool,
    trace_started: StdInstant,
    trace_sequence: AtomicU64,
    trace_entries: StdMutex<VecDeque<RecoveryTraceEntry>>,
    latest_startup_failures: StdMutex<HashMap<String, StartupFailure>>,
    trace_flush: std::sync::mpsc::SyncSender<()>,
}

#[derive(Debug, Clone, Serialize)]
struct StartupFailure {
    stage: &'static str,
    errno: Option<i32>,
    elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
struct RecoveryTraceEntry {
    sequence: u64,
    elapsed_ms: u64,
    stage: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    route: Option<SshRouteKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    child_exit_code: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    child_signal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    master_exit: Option<process::MasterExitReport>,
}

#[derive(Serialize)]
struct RecoveryTraceFile<'a> {
    schema_version: &'static str,
    attempts: &'a VecDeque<RecoveryTraceEntry>,
    latest_startup_failures: &'a HashMap<String, StartupFailure>,
}

struct SupervisorRecord {
    id: u64,
    task: JoinHandle<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SshConnectionObservation {
    status: SshConnectionStatus,
    generation: Option<u64>,
    route: Option<SshRouteKind>,
}

#[derive(Debug, Clone)]
struct CachedWanRoute {
    addresses: Vec<Ipv4Addr>,
    expires_at: StdInstant,
}

impl Drop for ManagerInner {
    fn drop(&mut self) {
        let supervisors = self
            .supervisors
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for supervisor in supervisors.values() {
            supervisor.task.abort();
        }
        supervisors.clear();
        let entries = self
            .entries
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (target_id, entry) in entries.iter() {
            if let Ok(mut entry) = entry.try_lock() {
                let previous = std::mem::replace(&mut entry.state, ConnectionState::Dormant);
                if let ConnectionState::Online(online) = previous {
                    let control_socket = online.spec.control_socket.clone();
                    drop(online);
                    cleanup_control_socket(&control_socket);
                }
                let target_dir = target_runtime_directory(&self.runtime_root, target_id);
                cleanup_receipt(&target_dir.join(RECEIPT_NAME));
                let _ = fs::remove_dir(target_dir);
            }
        }
        let _ = fs::remove_dir(&self.runtime_root);
        if let Some(parent) = self.runtime_root.parent() {
            let _ = fs::remove_dir(parent);
        }
    }
}

struct TargetEntry {
    next_generation: u64,
    state: ConnectionState,
    lan_upgrade_suppressed_until: Option<Instant>,
    candidate_gate: Arc<Mutex<()>>,
    network: Option<LocalNetworkSnapshot>,
    network_revision: u64,
}

enum ConnectionState {
    Dormant,
    Online(Box<OnlineConnection>),
    Recovering { attempt: usize, next_retry: Instant },
}

struct OnlineConnection {
    spec: MasterSpec,
    master: Box<dyn OwnedMaster>,
    retry_attempt: usize,
    connected_at: Instant,
}

impl OnlineConnection {
    fn failed_state(&self) -> ConnectionState {
        let prior = if self.connected_at.elapsed() >= STABLE_CONNECTION_RESET {
            0
        } else {
            self.retry_attempt
        };
        let attempt = prior.saturating_add(1).min(RETRY_DELAYS.len() + 1);
        // Preserve immediate recovery for a single failure, but not a chain of
        // successful handshakes that never become stable connections.
        let delay = if prior == 0 {
            Duration::ZERO
        } else {
            retry_delay(prior)
        };
        tracing::info!(target: "polar_station_operation", component="SSH", event="ssh_retry_scheduled",
            target_id=self.spec.target_id.as_str(), generation=self.spec.generation,
            attempt, retry_delay_ms=delay.as_millis() as u64);
        ConnectionState::Recovering {
            attempt,
            next_retry: Instant::now() + delay,
        }
    }
}

/// Keeps the superseded master alive long enough for generation consumers to
/// replace their child channel, while still guaranteeing cleanup if the
/// retirement task is cancelled during Host shutdown.
struct RetiringOnlineConnection {
    online: Option<OnlineConnection>,
}

impl RetiringOnlineConnection {
    fn after_grace(online: OnlineConnection) {
        let retire_at = Instant::now() + OLD_ROUTE_RETIRE_GRACE;
        let retiring = Self {
            online: Some(online),
        };
        tokio::spawn(async move {
            tokio::time::sleep_until(retire_at).await;
            drop(retiring);
        });
    }
}

impl Drop for RetiringOnlineConnection {
    fn drop(&mut self) {
        if let Some(online) = self.online.take() {
            let control_socket = online.spec.control_socket.clone();
            drop(online);
            cleanup_control_socket(&control_socket);
        }
    }
}

#[derive(Debug, Serialize)]
struct OwnershipReceipt<'a> {
    target_id: &'a str,
    generation: u64,
    master_pid: u32,
    destination: &'a str,
    destination_fingerprint: &'a str,
    control_socket: &'a Path,
    route: SshRouteKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    route_profile: Option<&'a SshRouteProfile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedReceipt {
    target_id: String,
    generation: u64,
    master_pid: u32,
    destination: String,
    destination_fingerprint: String,
    control_socket: PathBuf,
    route: SshRouteKind,
    #[serde(default)]
    route_profile: Option<SshRouteProfile>,
}

impl SshConnectionManager {
    pub(crate) fn system(runtime_root: PathBuf) -> Self {
        let manager = Self::with_backends(
            runtime_root,
            Arc::new(OpenSshBackend::system()),
            Arc::new(SystemNetworkRouteBackend),
        );
        spawn_platform_network_monitor(Arc::downgrade(&manager.inner));
        manager
    }

    

    fn with_backends(
        runtime_root: PathBuf,
        backend: Arc<dyn SshMasterBackend>,
        route_backend: Arc<dyn NetworkRouteBackend>,
    ) -> Self {
        let (status_events, _) = broadcast::channel(64);
        let (network_events, _) = broadcast::channel(16);
        let (trace_flush, flush_requests) = std::sync::mpsc::sync_channel(1);
        let manager = Self {
            inner: Arc::new(ManagerInner {
                instance_id: NEXT_MANAGER_INSTANCE_ID.fetch_add(1, Ordering::Relaxed),
                runtime_root,
                backend,
                route_backend,
                entries: StdMutex::new(HashMap::new()),
                observed_statuses: StdMutex::new(HashMap::new()),
                verified_leases: StdMutex::new(HashMap::new()),
                status_events,
                network_events,
                wan_cache: StdMutex::new(HashMap::new()),
                next_operation_id: AtomicU64::new(1),
                next_supervisor_id: AtomicU64::new(1),
                supervisors: StdMutex::new(HashMap::new()),
                stopping: AtomicBool::new(false),
                trace_flush,
                trace_started: StdInstant::now(),
                trace_sequence: AtomicU64::new(1),
                trace_entries: StdMutex::new(VecDeque::with_capacity(RECOVERY_TRACE_CAPACITY)),
                latest_startup_failures: StdMutex::new(HashMap::new()),
            }),
        };
        let weak = Arc::downgrade(&manager.inner);
        // One writer per manager, no unbounded tasks or queue, and no strong
        // reference keeping a retired manager alive while waiting for work.
        let _ = std::thread::Builder::new()
            .name("station-recovery-trace".into())
            .spawn(move || {
                while flush_requests.recv().is_ok() {
                    std::thread::sleep(Duration::from_millis(100));
                    while flush_requests.try_recv().is_ok() {}
                    let Some(inner) = weak.upgrade() else {
                        break;
                    };
                    flush_recovery_trace(&inner);
                }
            });
        manager
    }

    /// Ensure exactly one Rust-owned liveness supervisor is running for a
    /// required SSH target. The supervisor owns retry timing and observes the
    /// verified master generation; native adapters only consume its events.
    pub(crate) fn ensure_supervisor(
        &self,
        target_id: &str,
        destination: &str,
    ) -> Result<(), SshConnectionError> {
        self.ensure_supervisor_internal(target_id, destination, None)
    }

    pub(crate) fn ensure_supervisor_routed(
        &self,
        target_id: &str,
        destination: &str,
        profile: SshRouteProfile,
    ) -> Result<(), SshConnectionError> {
        if !profile.validate() {
            return Err(SshConnectionError::new(
                SshConnectionErrorCode::InvalidTarget,
                "the configured SSH route profile is invalid",
            ));
        }
        self.ensure_supervisor_internal(target_id, destination, Some(profile))
    }

    fn ensure_supervisor_internal(
        &self,
        target_id: &str,
        destination: &str,
        profile: Option<SshRouteProfile>,
    ) -> Result<(), SshConnectionError> {
        validate_target(target_id, destination)?;
        self.record_initial_status(target_id);
        let mut supervisors = self
            .inner
            .supervisors
            .lock()
            .expect("SSH supervisor map poisoned");
        if self.inner.stopping.load(Ordering::Acquire) {
            return Err(master_unavailable());
        }
        if supervisors.contains_key(target_id) {
            return Ok(());
        }
        let supervisor_id = self
            .inner
            .next_supervisor_id
            .fetch_add(1, Ordering::Relaxed);
        let manager = self.clone();
        let owned_target_id = target_id.to_owned();
        let owned_destination = destination.to_owned();
        let task = tokio::spawn(async move {
            loop {
                let result = std::panic::AssertUnwindSafe(manager.run_supervisor_guard(
                    &owned_target_id,
                    &owned_destination,
                    profile.as_ref(),
                ))
                .catch_unwind()
                .await;
                if result.is_ok() {
                    manager.clear_supervisor(&owned_target_id, supervisor_id);
                    break;
                }
                tracing::error!(
                    execution_target_id = %owned_target_id,
                    "SSH supervisor panicked; rearming target demand"
                );
                tokio::time::sleep(SUPERVISOR_REARM_DELAY).await;
            }
        });
        supervisors.insert(
            target_id.to_owned(),
            SupervisorRecord {
                id: supervisor_id,
                task,
            },
        );
        Ok(())
    }

    async fn run_supervisor_guard(
        &self,
        target_id: &str,
        destination: &str,
        profile: Option<&SshRouteProfile>,
    ) {
        loop {
            let terminal_error = self.run_supervisor(target_id, destination, profile).await;
            let Some(error) = terminal_error else {
                tracing::error!(
                    execution_target_id = %target_id,
                    "SSH supervisor loop ended without a terminal error; rearming demand"
                );
                tokio::time::sleep(SUPERVISOR_REARM_DELAY).await;
                continue;
            };

            if supervisor_error_is_fail_closed(error.code()) {
                tracing::warn!(
                    execution_target_id = %target_id,
                    %error,
                    "SSH supervisor stopped with a fail-closed target error"
                );
                return;
            }

            tracing::warn!(
                execution_target_id = %target_id,
                %error,
                "SSH supervisor loop ended; rearming target demand"
            );
            tokio::time::sleep(SUPERVISOR_REARM_DELAY).await;
        }
    }

    async fn run_supervisor(
        &self,
        target_id: &str,
        destination: &str,
        profile: Option<&SshRouteProfile>,
    ) -> Option<SshConnectionError> {
        let mut network_events = self.inner.network_events.subscribe();
        let mut route_audit = tokio::time::interval(ROUTE_AUDIT_INTERVAL);
        route_audit.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        route_audit.tick().await;
        self.refresh_network_state(target_id, false).await;
        loop {
            let lease = match self
                .recover_until_connected_internal(target_id, destination, profile)
                .await
            {
                Ok(lease) => lease,
                Err(error) if error.code().retryable() => continue,
                Err(error) => return Some(error),
            };
            let mut exited = {
                let entry = self.entry(target_id);
                let entry = entry.lock().await;
                match &entry.state {
                    ConnectionState::Online(online)
                        if online.spec.generation == lease.generation =>
                    {
                        online.master.exit_notification()
                    }
                    _ => continue,
                }
            };
            let stable_deadline = Instant::now() + STABLE_CONNECTION_RESET;
            let mut stable = false;
            loop {
                let network_wake = tokio::select! {
                    _ = wait_for_master_exit(&mut exited) => {
                        self.record_master_exit(target_id, lease.generation).await;
                        let _ = self.retire_route_generation(&lease, lease.route() == SshRouteKind::Lan, true).await;
                        break;
                    },
                    _ = tokio::time::sleep_until(stable_deadline), if !stable => {
                        stable = true;
                        self.reset_retry_state(target_id).await;
                        continue;
                    },
                    _ = route_audit.tick(), if profile.is_some() => {
                        self.record_terminal_route_stage("route_audit", target_id, lease.generation);
                        false
                    },
                    event = network_events.recv() => {
                        if matches!(event, Err(broadcast::error::RecvError::Closed)) { return None; }
                        true
                    },
                };
                let Some(profile) = profile else {
                    continue;
                };
                self.refresh_network_state(target_id, network_wake).await;
                if network_wake {
                    self.record_terminal_route_stage(
                        "network_reconciled",
                        target_id,
                        lease.generation,
                    );
                }
                match lease.route() {
                    SshRouteKind::Wan => {
                        let upgrade = self.try_lan_upgrade(target_id, destination, profile, &lease);
                        tokio::pin!(upgrade);
                        let result = loop {
                            tokio::select! {
                                result = &mut upgrade => break result,
                                _ = wait_for_master_exit(&mut exited) => break Err(master_unavailable()),
                                event = network_events.recv() => {
                                    if event.is_err() || self.refresh_network_state(target_id, true).await {
                                        // Reconcile the newer event immediately after cancelling the stale candidate.
                                        route_audit.reset_immediately();
                                        break Err(master_unavailable());
                                    }
                                },
                            }
                        };
                        if result.is_ok() {
                            break;
                        }
                        if *exited.borrow() {
                            self.record_master_exit(target_id, lease.generation).await;
                            let _ = self.retire_route_generation(&lease, false, true).await;
                            break;
                        }
                    }
                    SshRouteKind::Lan => {
                        if !self
                            .inner
                            .route_backend
                            .lan_is_direct(profile.lan_address)
                            .unwrap_or(false)
                        {
                            let _ = self.retire_route_generation(&lease, true, false).await;
                            break;
                        }
                    }
                    SshRouteKind::Opaque => {}
                }
            }
        }
    }

    async fn refresh_network_state(&self, target_id: &str, force: bool) -> bool {
        let Ok(network) = self.inner.route_backend.local_network() else {
            return false;
        };
        let entry = self.entry(target_id);
        let mut entry = entry.lock().await;
        if !force && entry.network.as_ref() == Some(&network) {
            return false;
        }
        let initialized = entry.network.is_some();
        entry.network = Some(network);
        if initialized || force {
            entry.network_revision = entry.network_revision.saturating_add(1);
            entry.lan_upgrade_suppressed_until = None;
        }
        if initialized {
            if let ConnectionState::Recovering {
                attempt,
                next_retry,
            } = &mut entry.state
            {
                *attempt = 0;
                *next_retry = Instant::now();
            }
        }
        true
    }

    fn record_terminal_route_stage(&self, stage: &'static str, target: &str, generation: u64) {
        tracing::info!(target: "polar_station_operation", component = "SSH", event = stage, target_id = target, generation);
        record_recovery_trace(&self.inner, stage, None, Some(generation), None);
    }

    async fn record_master_exit(&self, target: &str, generation: u64) {
        let entry = self.entry(target);
        let entry = entry.lock().await;
        let report = match &entry.state {
            ConnectionState::Online(online) if online.spec.generation == generation => {
                online.master.exit_report()
            }
            _ => None,
        };
        tracing::info!(target: "polar_station_operation", component="SSH", event="master_exit", target_id=target, generation);
        append_recovery_trace(
            &self.inner,
            RecoveryTraceEntry {
                sequence: self.inner.trace_sequence.fetch_add(1, Ordering::Relaxed),
                elapsed_ms: self
                    .inner
                    .trace_started
                    .elapsed()
                    .as_millis()
                    .try_into()
                    .unwrap_or(u64::MAX),
                stage: "master_exit",
                status: None,
                generation: Some(generation),
                route: None,
                child_exit_code: report.as_ref().and_then(|r| r.code),
                child_signal: report
                    .as_ref()
                    .and_then(|r| r.signal)
                    .map(|s| s.to_string()),
                target_id: Some(target.to_owned()),
                master_exit: report,
            },
        );
    }

    fn clear_supervisor(&self, target_id: &str, supervisor_id: u64) {
        let mut supervisors = self
            .inner
            .supervisors
            .lock()
            .expect("SSH supervisor map poisoned");
        if supervisors
            .get(target_id)
            .is_some_and(|record| record.id == supervisor_id)
        {
            supervisors.remove(target_id);
        }
    }

    async fn reset_retry_state(&self, target_id: &str) {
        let Some(entry) = self
            .inner
            .entries
            .lock()
            .expect("SSH manager entry map poisoned")
            .get(target_id)
            .cloned()
        else {
            return;
        };
        let mut entry = entry.lock().await;
        if let ConnectionState::Online(online) = &mut entry.state {
            if online.connected_at.elapsed() >= STABLE_CONNECTION_RESET {
                online.retry_attempt = 0;
            }
        }
    }

    pub(crate) async fn wait_for_connected_after(
        &self,
        target_id: &str,
        destination: &str,
        after: Option<SshTransportGenerationV1>,
    ) -> Result<SshTransportGenerationV1, SshConnectionError> {
        self.wait_for_connected_after_internal(target_id, destination, None, after)
            .await
    }

    pub(crate) async fn wait_for_connected_after_routed(
        &self,
        target_id: &str,
        destination: &str,
        profile: &SshRouteProfile,
        after: Option<SshTransportGenerationV1>,
    ) -> Result<SshTransportGenerationV1, SshConnectionError> {
        if !profile.validate() {
            return Err(SshConnectionError::new(
                SshConnectionErrorCode::InvalidTarget,
                "the configured SSH route profile is invalid",
            ));
        }
        self.wait_for_connected_after_internal(target_id, destination, Some(profile), after)
            .await
    }

    async fn wait_for_connected_after_internal(
        &self,
        target_id: &str,
        destination: &str,
        profile: Option<&SshRouteProfile>,
        after: Option<SshTransportGenerationV1>,
    ) -> Result<SshTransportGenerationV1, SshConnectionError> {
        validate_target(target_id, destination)?;
        let mut changes = self.subscribe_status_changes();
        self.ensure_supervisor_internal(target_id, destination, profile.cloned())?;
        if let Some(current) = self.transport_generation(target_id).await {
            if transport_generation_is_after(&current, after.as_ref()) {
                return Ok(current);
            }
        }
        loop {
            match changes.recv().await {
                Ok(change) if change.target_id == target_id => {
                    if change.status == SshConnectionStatus::Connected {
                        if let Some(generation) = change.generation {
                            let current = SshTransportGenerationV1 {
                                supervisor_instance_id: self.inner.instance_id,
                                generation,
                            };
                            if transport_generation_is_after(&current, after.as_ref()) {
                                return Ok(current);
                            }
                        }
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    if let Some(current) = self.transport_generation(target_id).await {
                        if transport_generation_is_after(&current, after.as_ref()) {
                            return Ok(current);
                        }
                    }
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(master_unavailable());
                }
            }
        }
    }

    pub(crate) async fn transport_generation(
        &self,
        target_id: &str,
    ) -> Option<SshTransportGenerationV1> {
        let generation = self
            .inner
            .verified_leases
            .lock()
            .expect("SSH verified lease map poisoned")
            .get(target_id)?
            .generation;
        Some(SshTransportGenerationV1 {
            supervisor_instance_id: self.inner.instance_id,
            generation,
        })
    }

    pub(crate) fn transport_generation_for_lease(
        &self,
        lease: &SshConnectionLease,
    ) -> SshTransportGenerationV1 {
        SshTransportGenerationV1 {
            supervisor_instance_id: self.inner.instance_id,
            generation: lease.generation(),
        }
    }

    pub(crate) async fn acquire(
        &self,
        target_id: &str,
        destination: &str,
    ) -> Result<SshConnectionLease, SshConnectionError> {
        self.acquire_internal(target_id, destination, None).await
    }

    pub(crate) async fn acquire_routed(
        &self,
        target_id: &str,
        destination: &str,
        profile: &SshRouteProfile,
    ) -> Result<SshConnectionLease, SshConnectionError> {
        if !profile.validate() {
            return Err(SshConnectionError::new(
                SshConnectionErrorCode::InvalidTarget,
                "the configured SSH route profile is invalid",
            ));
        }
        self.acquire_internal(target_id, destination, Some(profile))
            .await
    }

    pub(crate) async fn lease_if_connected(
        &self,
        target_id: &str,
        destination: &str,
        profile: Option<&SshRouteProfile>,
    ) -> Result<SshConnectionLease, SshConnectionError> {
        validate_target(target_id, destination)?;
        if profile.is_some_and(|profile| !profile.validate()) {
            return Err(SshConnectionError::new(
                SshConnectionErrorCode::InvalidTarget,
                "the configured SSH route profile is invalid",
            ));
        }
        let expected_fingerprint = route_destination_fingerprint(destination, profile);
        let mut lease = self
            .inner
            .verified_leases
            .lock()
            .expect("SSH verified lease map poisoned")
            .get(target_id)
            .cloned()
            .ok_or_else(master_unavailable)?;
        if lease.destination_fingerprint != expected_fingerprint {
            return Err(master_unavailable());
        }
        lease.operation_id = self.inner.next_operation_id.fetch_add(1, Ordering::Relaxed);
        Ok(lease)
    }

    async fn acquire_internal(
        &self,
        target_id: &str,
        destination: &str,
        profile: Option<&SshRouteProfile>,
    ) -> Result<SshConnectionLease, SshConnectionError> {
        validate_target(target_id, destination)?;
        self.refresh_network_state(target_id, false).await;
        self.record_initial_status(target_id);
        let target_dir = self.prepare_target_directory(target_id)?;
        let fingerprint = route_destination_fingerprint(destination, profile);
        let entry = self.entry(target_id);
        // Serialize candidate ownership, never the published state or healthy lease reads.
        let gate = entry.lock().await.candidate_gate.clone();
        let wait = tracing::info_span!(target: "polar_station_operation", "ssh_candidate_gate_wait", component="SSH", target_id);
        let _candidate = tracing::Instrument::instrument(gate.lock(), wait).await;
        let (generation, network_revision, previous_attempt, lan_allowed) = {
            let mut entry = entry.lock().await;
            if let ConnectionState::Online(online) = &mut entry.state {
                if online.spec.destination_fingerprint == fingerprint
                    && online.master.is_alive().unwrap_or(false)
                    && verify_control_socket(&online.spec.control_socket).is_ok()
                {
                    return Ok(self.lease(online));
                }
                cleanup_control_socket(&online.spec.control_socket);
                entry.state = online.failed_state();
                self.record_status(target_id, SshConnectionStatus::Recovering);
            }
            let previous_attempt = match entry.state {
                ConnectionState::Recovering {
                    attempt,
                    next_retry,
                } => {
                    if Instant::now() < next_retry {
                        return Err(master_unavailable());
                    }
                    attempt
                }
                _ => 0,
            };
            let state = (
                entry.next_generation.saturating_add(1),
                entry.network_revision,
                previous_attempt,
                entry
                    .lan_upgrade_suppressed_until
                    .is_none_or(|deadline| deadline <= Instant::now()),
            );
            entry.state = ConnectionState::Recovering {
                attempt: previous_attempt,
                next_retry: Instant::now(),
            };
            state
        };
        let span = tracing::info_span!(target: "polar_station_operation", "ssh_connect", component = "SSH", target_id, generation, network_revision, outcome = "cancelled");
        let candidate = tracing::Instrument::instrument(
            self.start_best_route(
                target_id,
                destination,
                generation,
                &target_dir,
                profile,
                lan_allowed,
            ),
            span.clone(),
        )
        .await;
        let candidate = prepare_candidate_receipt(candidate, &target_dir).await;
        let mut entry = entry.lock().await;
        if entry.network_revision != network_revision {
            span.record("outcome", "stale");
            return Err(master_unavailable());
        }
        let (mut online, receipt) = match candidate {
            Ok(candidate) => candidate,
            Err(error) => {
                let attempt = previous_attempt.saturating_add(1).min(RETRY_DELAYS.len());
                entry.state = ConnectionState::Recovering {
                    attempt,
                    next_retry: Instant::now() + retry_delay(attempt),
                };
                self.record_status(target_id, recovery_status(attempt));
                span.record("outcome", "failed");
                return Err(error);
            }
        };
        if !online.master.is_alive().unwrap_or(false) {
            span.record("outcome", "failed");
            return Err(master_unavailable());
        }
        if let Err(error) = receipt.publish() {
            cleanup_control_socket(&online.spec.control_socket);
            entry.state = ConnectionState::Recovering {
                attempt: previous_attempt.saturating_add(1),
                next_retry: Instant::now() + retry_delay(previous_attempt.saturating_add(1)),
            };
            span.record("outcome", "failed");
            return Err(error);
        }
        let lease = self.lease(&online);
        online.retry_attempt = previous_attempt;
        online.connected_at = Instant::now();
        entry.next_generation = generation;
        entry.state = ConnectionState::Online(Box::new(online));
        self.record_connected_lease(&lease, false);
        span.record("outcome", "ok");
        Ok(lease)
    }

    

    async fn recover_until_connected_internal(
        &self,
        target_id: &str,
        destination: &str,
        profile: Option<&SshRouteProfile>,
    ) -> Result<SshConnectionLease, SshConnectionError> {
        let mut network_events = self.inner.network_events.subscribe();
        loop {
            let attempt = self.acquire_internal(target_id, destination, profile);
            tokio::pin!(attempt);
            let result = loop {
                tokio::select! {
                    result = &mut attempt => break Some(result),
                    event = network_events.recv() => {
                        if event.is_err() || self.refresh_network_state(target_id, true).await { break None; }
                    },
                }
            };
            match result {
                Some(Ok(lease)) => return Ok(lease),
                Some(Err(error)) if !error.code().retryable() => return Err(error),
                None => continue,
                Some(Err(_)) => {}
            }
            let deadline = self
                .retry_deadline(target_id)
                .await
                .unwrap_or_else(Instant::now);
            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => break,
                    event = network_events.recv() => {
                        if event.is_err() || self.refresh_network_state(target_id, true).await { break; }
                    },
                }
            }
        }
    }

    pub(crate) fn subscribe_status_changes(
        &self,
    ) -> broadcast::Receiver<SshConnectionStatusChange> {
        self.inner.status_events.subscribe()
    }

    pub(crate) fn current_status_change(
        &self,
        target_id: &str,
    ) -> Option<SshConnectionStatusChange> {
        let observation = *self
            .inner
            .observed_statuses
            .lock()
            .expect("SSH manager status map poisoned")
            .get(target_id)?;
        Some(SshConnectionStatusChange {
            target_id: target_id.to_owned(),
            status: observation.status,
            generation: observation.generation,
            route: observation.route,
            preserves_consumer_identity: true,
        })
    }

    pub(crate) fn record_terminal_exit(&self, code: Option<u32>, signal: Option<String>) {
        append_recovery_trace(
            &self.inner,
            RecoveryTraceEntry {
                sequence: self.inner.trace_sequence.fetch_add(1, Ordering::Relaxed),
                elapsed_ms: self
                    .inner
                    .trace_started
                    .elapsed()
                    .as_millis()
                    .try_into()
                    .unwrap_or(u64::MAX),
                stage: "pane_child_exited_recheck_session",
                status: None,
                generation: None,
                route: None,
                child_exit_code: code,
                child_signal: signal,
                target_id: None,
                master_exit: None,
            },
        );
    }

    pub(crate) fn record_consumer_recovery_stage(&self, stage: &'static str) {
        record_recovery_trace(&self.inner, stage, None, None, None);
    }

    pub(crate) async fn validate(
        &self,
        lease: &SshConnectionLease,
    ) -> Result<(), SshConnectionError> {
        let entry = {
            self.inner
                .entries
                .lock()
                .expect("SSH manager entry map poisoned")
                .get(&lease.target_id)
                .cloned()
        }
        .ok_or_else(|| {
            self.record_status(&lease.target_id, SshConnectionStatus::Offline);
            SshConnectionError::new(
                SshConnectionErrorCode::GenerationStale,
                "the SSH master lease is no longer registered",
            )
        })?;
        let mut entry = entry.lock().await;
        let previous = std::mem::replace(&mut entry.state, ConnectionState::Dormant);
        let ConnectionState::Online(mut online) = previous else {
            entry.state = previous;
            self.record_status(&lease.target_id, SshConnectionStatus::Recovering);
            return Err(SshConnectionError::new(
                SshConnectionErrorCode::GenerationStale,
                "the SSH master lease generation is stale",
            ));
        };
        let identity_matches = online.spec.target_id == lease.target_id
            && online.spec.generation == lease.generation
            && online.master.pid() == lease.master_pid
            && online.spec.destination_fingerprint == lease.destination_fingerprint
            && online.spec.control_socket == lease.control_socket;
        if !identity_matches {
            let current_lease = self.lease(&online);
            entry.state = ConnectionState::Online(online);
            self.record_connected_lease(&current_lease, true);
            return Err(SshConnectionError::new(
                SshConnectionErrorCode::GenerationStale,
                "the requested SSH master lease is stale; the current generation remains healthy",
            ));
        }
        if self.verify_online(&mut online).is_err() {
            cleanup_control_socket(&online.spec.control_socket);
            entry.state = online.failed_state();
            self.record_status(&lease.target_id, SshConnectionStatus::Recovering);
            return Err(SshConnectionError::new(
                SshConnectionErrorCode::MasterUnavailable,
                "the shared SSH master is unavailable; retry the operation",
            ));
        }
        let current_lease = self.lease(&online);
        entry.state = ConnectionState::Online(online);
        self.record_connected_lease(&current_lease, false);
        Ok(())
    }

    pub(crate) fn aggregate_status<'a>(
        &self,
        target_ids: impl IntoIterator<Item = &'a str>,
    ) -> SshConnectionStatus {
        let statuses = self
            .inner
            .observed_statuses
            .lock()
            .expect("SSH manager status map poisoned");
        let mut aggregate = SshConnectionStatus::Connected;
        for target_id in target_ids {
            let status = statuses
                .get(target_id)
                .map(|observation| observation.status)
                .unwrap_or(SshConnectionStatus::Connecting);
            aggregate = less_healthy_status(aggregate, status);
        }
        aggregate
    }

    pub(crate) async fn retire(&self, target_id: &str) {
        let supervisor = self
            .inner
            .supervisors
            .lock()
            .expect("SSH supervisor map poisoned")
            .remove(target_id);
        if let Some(supervisor) = supervisor {
            supervisor.task.abort();
            let _ = supervisor.task.await;
        }
        let entry = self
            .inner
            .entries
            .lock()
            .expect("SSH manager entry map poisoned")
            .remove(target_id);
        self.inner
            .observed_statuses
            .lock()
            .expect("SSH manager status map poisoned")
            .remove(target_id);
        self.inner
            .verified_leases
            .lock()
            .expect("SSH verified lease map poisoned")
            .remove(target_id);
        self.inner
            .latest_startup_failures
            .lock()
            .expect("SSH startup failure map poisoned")
            .remove(target_id);
        if let Some(entry) = entry {
            let mut entry = entry.lock().await;
            if let ConnectionState::Online(online) =
                std::mem::replace(&mut entry.state, ConnectionState::Dormant)
            {
                cleanup_control_socket(&online.spec.control_socket);
            }
        }
        let target_dir = target_runtime_directory(&self.inner.runtime_root, target_id);
        cleanup_receipt(&target_dir.join(RECEIPT_NAME));
        let _ = fs::remove_dir(target_dir);
    }

    /// Stop demand before dropping the async runtime; no user Session is killed.
    pub(crate) async fn shutdown(&self) {
        self.inner.stopping.store(true, Ordering::Release);
        let targets: Vec<_> = self
            .inner
            .entries
            .lock()
            .expect("SSH entry map")
            .keys()
            .cloned()
            .collect();
        for target in targets {
            self.retire(&target).await;
        }
    }

    fn record_status(&self, target_id: &str, status: SshConnectionStatus) {
        self.record_status_with_generation_and_route(target_id, status, None, None, false);
    }

    fn record_connected_lease(
        &self,
        lease: &SshConnectionLease,
        preserves_consumer_identity: bool,
    ) {
        self.inner
            .verified_leases
            .lock()
            .expect("SSH verified lease map poisoned")
            .insert(lease.target_id.clone(), lease.clone());
        self.record_status_with_generation_and_route(
            &lease.target_id,
            SshConnectionStatus::Connected,
            Some(lease.generation),
            Some(lease.route),
            preserves_consumer_identity,
        );
    }

    fn record_status_with_generation_and_route(
        &self,
        target_id: &str,
        status: SshConnectionStatus,
        generation: Option<u64>,
        route: Option<SshRouteKind>,
        preserves_consumer_identity: bool,
    ) {
        if status != SshConnectionStatus::Connected {
            self.inner
                .verified_leases
                .lock()
                .expect("SSH verified lease map poisoned")
                .remove(target_id);
        }
        let observation = SshConnectionObservation {
            status,
            generation,
            route,
        };
        let changed = self
            .inner
            .observed_statuses
            .lock()
            .expect("SSH manager status map poisoned")
            .insert(target_id.to_owned(), observation)
            != Some(observation);
        if changed {
            record_recovery_trace(
                &self.inner,
                "generation_state_published",
                Some(status),
                generation,
                route,
            );
            let _ = self.inner.status_events.send(SshConnectionStatusChange {
                target_id: target_id.to_owned(),
                status,
                generation,
                route,
                preserves_consumer_identity,
            });
        }
    }

    fn record_initial_status(&self, target_id: &str) {
        let inserted = {
            let mut statuses = self
                .inner
                .observed_statuses
                .lock()
                .expect("SSH manager status map poisoned");
            if statuses.contains_key(target_id) {
                false
            } else {
                statuses.insert(
                    target_id.to_owned(),
                    SshConnectionObservation {
                        status: SshConnectionStatus::Connecting,
                        generation: None,
                        route: None,
                    },
                );
                true
            }
        };
        if inserted {
            let _ = self.inner.status_events.send(SshConnectionStatusChange {
                target_id: target_id.to_owned(),
                status: SshConnectionStatus::Connecting,
                generation: None,
                route: None,
                preserves_consumer_identity: false,
            });
        }
    }

    async fn retry_deadline(&self, target_id: &str) -> Option<Instant> {
        let entry = self
            .inner
            .entries
            .lock()
            .expect("SSH manager entry map poisoned")
            .get(target_id)
            .cloned()?;
        let entry = entry.lock().await;
        match &entry.state {
            ConnectionState::Recovering { next_retry, .. } => Some(*next_retry),
            ConnectionState::Dormant | ConnectionState::Online(_) => None,
        }
    }

    fn entry(&self, target_id: &str) -> Arc<Mutex<TargetEntry>> {
        self.inner
            .entries
            .lock()
            .expect("SSH manager entry map poisoned")
            .entry(target_id.to_string())
            .or_insert_with(|| {
                Arc::new(Mutex::new(TargetEntry {
                    next_generation: 0,
                    state: ConnectionState::Dormant,
                    lan_upgrade_suppressed_until: None,
                    candidate_gate: Arc::new(Mutex::new(())),
                    network: None,
                    network_revision: 0,
                }))
            })
            .clone()
    }

    async fn start_best_route(
        &self,
        target_id: &str,
        destination: &str,
        generation: u64,
        target_dir: &Path,
        profile: Option<&SshRouteProfile>,
        lan_candidate_allowed: bool,
    ) -> Result<OnlineConnection, SshConnectionError> {
        let Some(profile) = profile else {
            return self
                .start_route(
                    target_id,
                    destination,
                    generation,
                    target_dir,
                    None,
                    SshRoute::Opaque,
                )
                .await;
        };
        let lan_is_direct = self
            .inner
            .route_backend
            .lan_is_direct(profile.lan_address)
            .unwrap_or(false);
        let resolution = self.resolve_wan_cached(profile);
        tokio::pin!(resolution);
        let resolved = if lan_candidate_allowed && lan_is_direct {
            let candidate = self.start_route(
                target_id,
                destination,
                generation,
                target_dir,
                Some(profile),
                lan_route(profile),
            );
            tokio::pin!(candidate);
            // DNS can overlap LAN, but WAN authentication never races a viable LAN candidate.
            // Both futures belong to this attempt and are dropped on cancellation/success.
            tokio::select! {
                biased;
                online = &mut candidate => match online {
                    Ok(online) => return Ok(online),
                    Err(_) => resolution.await?,
                },
                resolved = &mut resolution => {
                    if let Ok(online) = candidate.await { return Ok(online); }
                    resolved?
                },
            }
        } else {
            resolution.await?
        };
        for address in resolved {
            if let Ok(online) = self
                .start_route(
                    target_id,
                    destination,
                    generation,
                    target_dir,
                    Some(profile),
                    wan_route(profile, address),
                )
                .await
            {
                return Ok(online);
            }
        }
        Err(master_unavailable())
    }

    async fn start_route(
        &self,
        target_id: &str,
        destination: &str,
        generation: u64,
        target_dir: &Path,
        route_profile: Option<&SshRouteProfile>,
        route: SshRoute,
    ) -> Result<OnlineConnection, SshConnectionError> {
        if !route_profile_matches_kind(route.kind(), route_profile) {
            return Err(SshConnectionError::new(
                SshConnectionErrorCode::InvalidTarget,
                "the SSH route does not match its configured profile",
            ));
        }
        let attempt_id = self.inner.next_operation_id.fetch_add(1, Ordering::Relaxed);
        let control_socket = target_dir.join(control_socket_name(
            generation,
            route.socket_tag(),
            Some(attempt_id),
        ));
        let control_socket_string = control_socket.to_string_lossy();
        if !polar_tmux_protocol::is_canonical_ssh_control_socket(&control_socket_string)
            || control_socket_string.len() + OPENSSH_TEMP_SUFFIX_BUDGET
                > MAX_CONTROL_SOCKET_PATH_LEN
        {
            return Err(SshConnectionError::new(
                SshConnectionErrorCode::InvalidTarget,
                "the private SSH control socket path is invalid or too long",
            ));
        }
        prepare_control_socket(&control_socket)?;
        let timeout = if route.kind() == SshRouteKind::Lan {
            LAN_CANDIDATE_TIMEOUT
        } else {
            STARTUP_TIMEOUT
        };
        tokio::time::timeout(
            timeout,
            self.start_and_verify(MasterSpec {
                target_id: target_id.to_owned(),
                destination: destination.to_owned(),
                destination_fingerprint: route_destination_fingerprint(destination, route_profile),
                generation,
                control_socket,
                route_profile: route_profile.cloned(),
                route,
            }),
        )
        .await
        .map_err(|_| master_unavailable())?
    }

    #[tracing::instrument(
        target = "polar_station_operation",
        name = "wan_resolution",
        skip_all,
        fields(component = "SSH", outcome = "cancelled")
    )]
    async fn resolve_wan_cached(
        &self,
        profile: &SshRouteProfile,
    ) -> Result<Vec<Ipv4Addr>, SshConnectionError> {
        if let Some(cached) = self
            .inner
            .wan_cache
            .lock()
            .expect("WAN route cache poisoned")
            .get(&profile.wan_host)
            .filter(|cached| cached.expires_at > StdInstant::now())
            .cloned()
        {
            tracing::info!(target: "polar_station_operation", component="SSH", event="wan_dns_cache_hit");
            tracing::Span::current().record("outcome", "cached");
            return Ok(cached.addresses);
        }
        tracing::info!(target: "polar_station_operation", component="SSH", event="wan_dns_execute");
        let resolved = self
            .inner
            .route_backend
            .resolve_wan(profile)
            .await
            .map_err(|_| {
                tracing::Span::current().record("outcome", "failed");
                master_unavailable()
            })?;
        if resolved.addresses.is_empty() {
            tracing::Span::current().record("outcome", "failed");
            return Err(master_unavailable());
        }
        self.inner
            .wan_cache
            .lock()
            .expect("WAN route cache poisoned")
            .insert(
                profile.wan_host.clone(),
                CachedWanRoute {
                    addresses: resolved.addresses.clone(),
                    expires_at: StdInstant::now() + resolved.ttl,
                },
            );
        tracing::Span::current().record("outcome", "ok");
        tracing::info!(target: "polar_station_operation", component="SSH", event="wan_dns_completed", count=resolved.addresses.len());
        Ok(resolved.addresses)
    }

    async fn try_lan_upgrade(
        &self,
        target_id: &str,
        destination: &str,
        profile: &SshRouteProfile,
        current_lease: &SshConnectionLease,
    ) -> Result<(), SshConnectionError> {
        if !self
            .inner
            .route_backend
            .lan_is_direct(profile.lan_address)
            .unwrap_or(false)
        {
            return Err(master_unavailable());
        }
        let target_dir = self.prepare_target_directory(target_id)?;
        let entry = self.entry(target_id);
        let gate = entry.lock().await.candidate_gate.clone();
        let wait = tracing::info_span!(target: "polar_station_operation", "ssh_candidate_gate_wait", component="SSH", target_id);
        let _candidate = tracing::Instrument::instrument(gate.lock(), wait).await;
        let (generation, network_revision) = {
            let entry = entry.lock().await;
            if entry
                .lan_upgrade_suppressed_until
                .is_some_and(|deadline| deadline > Instant::now())
            {
                return Err(master_unavailable());
            }
            let ConnectionState::Online(online) = &entry.state else {
                return Err(master_unavailable());
            };
            if online.spec.generation != current_lease.generation
                || online.spec.route.kind() != SshRouteKind::Wan
            {
                return Err(master_unavailable());
            }
            (
                entry.next_generation.saturating_add(1),
                entry.network_revision,
            )
        };
        let span = tracing::info_span!(target: "polar_station_operation", "lan_candidate", component = "SSH", target_id, generation, network_revision, outcome = "cancelled");
        let candidate = tracing::Instrument::instrument(
            self.start_route(
                target_id,
                destination,
                generation,
                &target_dir,
                Some(profile),
                lan_route(profile),
            ),
            span.clone(),
        )
        .await;
        let candidate = prepare_candidate_receipt(candidate, &target_dir).await;
        let mut entry = entry.lock().await;
        if entry.network_revision != network_revision
            || !matches!(&entry.state, ConnectionState::Online(online) if online.spec.generation == current_lease.generation && online.spec.route.kind() == SshRouteKind::Wan)
        {
            span.record("outcome", "stale");
            return Err(master_unavailable());
        }
        let (mut online, receipt) = match candidate {
            Ok(candidate) => candidate,
            Err(error) => {
                entry.lan_upgrade_suppressed_until = Some(Instant::now() + LAN_UPGRADE_HOLDOFF);
                span.record("outcome", "failed");
                return Err(error);
            }
        };
        if !online.master.is_alive().unwrap_or(false) {
            span.record("outcome", "failed");
            return Err(master_unavailable());
        }
        receipt.publish()?;
        let lease = self.lease(&online);
        let old = std::mem::replace(&mut entry.state, ConnectionState::Online(Box::new(online)));
        entry.next_generation = generation;
        entry.lan_upgrade_suppressed_until = None;
        if let ConnectionState::Online(old) = old {
            RetiringOnlineConnection::after_grace(*old);
        }
        self.record_connected_lease(&lease, true);
        span.record("outcome", "ok");
        Ok(())
    }

    async fn retire_route_generation(
        &self,
        lease: &SshConnectionLease,
        suppress_lan_upgrade: bool,
        master_failed: bool,
    ) -> Result<(), SshConnectionError> {
        let entry = self
            .inner
            .entries
            .lock()
            .expect("SSH manager entry map poisoned")
            .get(&lease.target_id)
            .cloned()
            .ok_or_else(master_unavailable)?;
        let mut entry = entry.lock().await;
        let previous = std::mem::replace(&mut entry.state, ConnectionState::Dormant);
        let ConnectionState::Online(online) = previous else {
            entry.state = previous;
            return Err(SshConnectionError::new(
                SshConnectionErrorCode::GenerationStale,
                "the SSH route generation changed before network recovery",
            ));
        };
        let identity_matches = online.spec.target_id == lease.target_id
            && online.spec.generation == lease.generation
            && online.master.pid() == lease.master_pid
            && online.spec.destination_fingerprint == lease.destination_fingerprint
            && online.spec.control_socket == lease.control_socket;
        if !identity_matches {
            entry.state = ConnectionState::Online(online);
            return Err(SshConnectionError::new(
                SshConnectionErrorCode::GenerationStale,
                "the SSH route generation changed before network recovery",
            ));
        }
        let old_socket = online.spec.control_socket.clone();
        let next_state = if master_failed {
            online.failed_state()
        } else {
            ConnectionState::Recovering {
                attempt: online.retry_attempt,
                next_retry: Instant::now(),
            }
        };
        drop(online);
        cleanup_control_socket(&old_socket);
        entry.state = next_state;
        if suppress_lan_upgrade {
            entry.lan_upgrade_suppressed_until = Some(Instant::now() + LAN_UPGRADE_HOLDOFF);
        }
        self.record_status(&lease.target_id, SshConnectionStatus::Recovering);
        Ok(())
    }

    

    

    async fn start_and_verify(
        &self,
        spec: MasterSpec,
    ) -> Result<OnlineConnection, SshConnectionError> {
        let readiness =
            socket_readiness::SocketReadiness::new(&spec.control_socket).map_err(|error| {
                record_startup_failure(
                    &self.inner,
                    &spec.target_id,
                    "socket_readiness_register",
                    error.raw_os_error(),
                );
                master_unavailable()
            })?;
        tracing::info!(target: "polar_station_operation", component="SSH", event="ssh_master_spawn_execute", target_id=spec.target_id.as_str(), generation=spec.generation);
        let mut master = self.inner.backend.spawn_master(&spec).map_err(|error| {
            record_startup_failure(
                &self.inner,
                &spec.target_id,
                "master_spawn",
                error.raw_os_error(),
            );
            master_unavailable()
        })?;
        tracing::info!(target: "polar_station_operation", component="SSH", event="ssh_master_spawned", target_id=spec.target_id.as_str(), generation=spec.generation, child_pid=master.pid());
        let mut exited = master.exit_notification();
        let wait = async {
            loop {
                if !master.is_alive().map_err(|error| {
                    record_startup_failure(
                        &self.inner,
                        &spec.target_id,
                        "master_liveness",
                        error.raw_os_error(),
                    );
                    master_unavailable()
                })? {
                    record_startup_failure(
                        &self.inner,
                        &spec.target_id,
                        "master_exited_before_ready",
                        None,
                    );
                    return Err(master_unavailable());
                }
                tracing::info!(target: "polar_station_operation", component="SSH", event="ssh_socket_inspect", target_id=spec.target_id.as_str(), generation=spec.generation);
                if verify_control_socket(&spec.control_socket).is_ok() {
                    tracing::info!(target: "polar_station_operation", component="SSH", event="ssh_control_socket_observed");
                    tracing::info!(target: "polar_station_operation", component="SSH", event="ssh_master_check_execute", target_id=spec.target_id.as_str(), generation=spec.generation);
                    self.inner
                        .backend
                        .check_master(&spec)
                        .await
                        .map_err(|error| {
                            record_startup_failure(
                                &self.inner,
                                &spec.target_id,
                                "master_check",
                                error.raw_os_error(),
                            );
                            master_unavailable()
                        })?;
                    tracing::info!(target: "polar_station_operation", component="SSH", event="ssh_master_verified");
                    return Ok(());
                }
                tokio::select! {
                    _ = wait_for_master_exit(&mut exited) => {
                        record_startup_failure(&self.inner, &spec.target_id, "master_exited_before_ready", None);
                        return Err(master_unavailable());
                    },
                    changed = readiness.changed() => { changed.map_err(|error| {
                        record_startup_failure(&self.inner, &spec.target_id, "socket_readiness_wait", error.raw_os_error());
                        master_unavailable()
                    })?; },
                }
                tracing::info!(target: "polar_station_operation", component="SSH", event="ssh_socket_wake", target_id=spec.target_id.as_str(), generation=spec.generation);
            }
        };
        tokio::time::timeout(STARTUP_TIMEOUT, wait)
            .await
            .map_err(|_| {
                record_startup_failure(&self.inner, &spec.target_id, "startup_deadline", None);
                master_unavailable()
            })??;
        Ok(OnlineConnection {
            spec,
            master,
            retry_attempt: 0,
            connected_at: Instant::now(),
        })
    }

    fn verify_online(&self, online: &mut OnlineConnection) -> io::Result<()> {
        if !online.master.is_alive()? {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "SSH master exited",
            ));
        }
        // This generation already passed check_master before publication. The
        // owned child and exact private socket are the ongoing liveness facts.
        verify_control_socket(&online.spec.control_socket)
    }

    fn lease(&self, online: &OnlineConnection) -> SshConnectionLease {
        SshConnectionLease {
            target_id: online.spec.target_id.clone(),
            generation: online.spec.generation,
            master_pid: online.master.pid(),
            destination: online.spec.destination.clone(),
            destination_fingerprint: online.spec.destination_fingerprint.clone(),
            control_socket: online.spec.control_socket.clone(),
            route: online.spec.route.kind(),
            operation_id: self.inner.next_operation_id.fetch_add(1, Ordering::Relaxed),
            exited: online.master.exit_notification(),
        }
    }

    fn prepare_target_directory(&self, target_id: &str) -> Result<PathBuf, SshConnectionError> {
        create_private_directory(&self.inner.runtime_root)?;
        let target_dir = target_runtime_directory(&self.inner.runtime_root, target_id);
        create_private_directory(&target_dir)?;
        Ok(target_dir)
    }
}

pub(crate) fn platform_ssh_runtime_root() -> PathBuf {
    // Explicit test/runtime isolation applies on macOS too. Its default TMPDIR
    // is too long for OpenSSH; callers must choose a short private XDG root.
    if let Some(runtime_root) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime_root).join("polar-tmux").join("ssh");
    }
    #[cfg(target_os = "macos")]
    {
        // macOS TMPDIR paths are long enough that OpenSSH's temporary
        // ControlMaster listener suffix can overflow sockaddr_un.
        // /tmp remains safe because every descendant is created owner-only
        // and revalidated before use.
        // SAFETY: geteuid has no arguments and is thread-safe.
        let uid = unsafe { libc::geteuid() };
        PathBuf::from("/tmp")
            .join(format!("polar-tmux-{uid}"))
            .join("ssh")
    }
    #[cfg(not(target_os = "macos"))]
    {
        #[cfg(unix)]
        let fallback = {
            // SAFETY: geteuid has no arguments and is thread-safe.
            let uid = unsafe { libc::geteuid() };
            std::env::temp_dir()
                .join(format!("polar-tmux-{uid}"))
                .join("ssh")
        };
        #[cfg(not(unix))]
        let fallback = std::env::temp_dir().join("polar-tmux").join("ssh");
        fallback
    }
}

fn validate_target(target_id: &str, destination: &str) -> Result<(), SshConnectionError> {
    let id_is_valid = !target_id.is_empty()
        && target_id.len() <= 64
        && target_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    if !id_is_valid || !polar_tmux_protocol::is_canonical_ssh_destination(destination) {
        return Err(SshConnectionError::new(
            SshConnectionErrorCode::InvalidTarget,
            "the configured SSH target is invalid",
        ));
    }
    Ok(())
}



fn route_destination_fingerprint(destination: &str, profile: Option<&SshRouteProfile>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(destination.as_bytes());
    if let Some(profile) = profile {
        hasher.update([0]);
        if let Ok(encoded) = serde_json::to_vec(profile) {
            hasher.update(encoded);
        }
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn route_profile_matches_kind(route: SshRouteKind, profile: Option<&SshRouteProfile>) -> bool {
    match (route, profile) {
        (SshRouteKind::Opaque, None) => true,
        (SshRouteKind::Lan | SshRouteKind::Wan, Some(profile)) => profile.validate(),
        (SshRouteKind::Opaque, Some(_)) | (SshRouteKind::Lan | SshRouteKind::Wan, None) => false,
    }
}

fn receipt_configuration_fingerprint_matches(
    destination: &str,
    fingerprint: &str,
    route: SshRouteKind,
    profile: Option<&SshRouteProfile>,
) -> bool {
    route_profile_matches_kind(route, profile)
        && fingerprint == route_destination_fingerprint(destination, profile)
}

fn target_runtime_directory(runtime_root: &Path, target_id: &str) -> PathBuf {
    let digest = Sha256::digest(target_id.as_bytes());
    let bounded_name = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    runtime_root.join(bounded_name)
}

fn retry_delay(attempt: usize) -> Duration {
    RETRY_DELAYS[attempt.saturating_sub(1).min(RETRY_DELAYS.len() - 1)]
}

fn transport_generation_is_after(
    current: &SshTransportGenerationV1,
    after: Option<&SshTransportGenerationV1>,
) -> bool {
    match after {
        None => true,
        Some(previous) if current.supervisor_instance_id != previous.supervisor_instance_id => true,
        Some(previous) => current.generation > previous.generation,
    }
}

fn recovery_status(attempt: usize) -> SshConnectionStatus {
    if attempt >= RETRY_DELAYS.len() {
        SshConnectionStatus::Offline
    } else {
        SshConnectionStatus::Recovering
    }
}

fn less_healthy_status(
    left: SshConnectionStatus,
    right: SshConnectionStatus,
) -> SshConnectionStatus {
    fn priority(status: SshConnectionStatus) -> u8 {
        match status {
            SshConnectionStatus::Connected => 0,
            SshConnectionStatus::Connecting => 1,
            SshConnectionStatus::Recovering => 2,
            SshConnectionStatus::Offline => 3,
        }
    }
    if priority(right) > priority(left) {
        right
    } else {
        left
    }
}

fn master_unavailable() -> SshConnectionError {
    SshConnectionError::new(
        SshConnectionErrorCode::MasterUnavailable,
        "the shared SSH master is unavailable; retry the operation",
    )
}

fn create_private_directory(path: &Path) -> Result<(), SshConnectionError> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ownership_conflict());
        }
    } else {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder.create(path).map_err(|_| ownership_conflict())?;
    }
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| ownership_conflict())?;
        let metadata = fs::symlink_metadata(path).map_err(|_| ownership_conflict())?;
        // SAFETY: geteuid has no arguments and is thread-safe.
        let uid = unsafe { libc::geteuid() };
        if metadata.uid() != uid
            || metadata.permissions().mode() & 0o077 != 0
            || metadata.file_type().is_symlink()
            || !metadata.is_dir()
        {
            return Err(ownership_conflict());
        }
    }
    Ok(())
}

fn prepare_control_socket(path: &Path) -> Result<(), SshConnectionError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            #[cfg(unix)]
            {
                // SAFETY: geteuid has no arguments and is thread-safe.
                let uid = unsafe { libc::geteuid() };
                if metadata.uid() != uid
                    || metadata.file_type().is_symlink()
                    || !metadata.file_type().is_socket()
                {
                    return Err(ownership_conflict());
                }
            }
            fs::remove_file(path).map_err(|_| ownership_conflict())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ownership_conflict()),
    }
}

fn verify_control_socket(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no arguments and is thread-safe.
        let uid = unsafe { libc::geteuid() };
        if metadata.uid() != uid
            || metadata.file_type().is_symlink()
            || !metadata.file_type().is_socket()
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "SSH control socket ownership is invalid",
            ));
        }
    }
    Ok(())
}

fn cleanup_control_socket(path: &Path) {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        #[cfg(unix)]
        {
            // SAFETY: geteuid has no arguments and is thread-safe.
            let uid = unsafe { libc::geteuid() };
            if metadata.uid() != uid
                || metadata.file_type().is_symlink()
                || !metadata.file_type().is_socket()
            {
                return;
            }
        }
        let _ = fs::remove_file(path);
    }
}

fn cleanup_receipt(path: &Path) {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        #[cfg(unix)]
        {
            // SAFETY: geteuid has no arguments and is thread-safe.
            let uid = unsafe { libc::geteuid() };
            if metadata.uid() != uid || metadata.file_type().is_symlink() || !metadata.is_file() {
                return;
            }
        }
        let _ = fs::remove_file(path);
    }
}

/// Only the short rename runs under the connection state lock. Dropping an
/// unpublished candidate removes its private temporary file, even on cancellation.
struct PreparedReceipt {
    temporary: PathBuf,
    final_path: PathBuf,
}

impl PreparedReceipt {
    fn write(
        temporary: PathBuf,
        final_path: PathBuf,
        bytes: Vec<u8>,
    ) -> Result<Self, SshConnectionError> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary).map_err(|_| ownership_conflict())?;
        let prepared = Self {
            temporary,
            final_path,
        };
        std::io::Write::write_all(&mut file, &bytes).map_err(|_| ownership_conflict())?;
        file.sync_all().map_err(|_| ownership_conflict())?;
        Ok(prepared)
    }

    fn publish(self) -> Result<(), SshConnectionError> {
        fs::rename(&self.temporary, &self.final_path).map_err(|_| ownership_conflict())
    }
}

impl Drop for PreparedReceipt {
    fn drop(&mut self) {
        cleanup_receipt(&self.temporary);
    }
}

#[tracing::instrument(
    target = "polar_station_operation",
    name = "ssh_receipt_prepare",
    skip_all,
    fields(component = "SSH")
)]
async fn prepare_candidate_receipt(
    candidate: Result<OnlineConnection, SshConnectionError>,
    target_dir: &Path,
) -> Result<(OnlineConnection, PreparedReceipt), SshConnectionError> {
    let online = candidate?;
    let bytes = serde_json::to_vec(&OwnershipReceipt {
        target_id: &online.spec.target_id,
        generation: online.spec.generation,
        master_pid: online.master.pid(),
        destination: &online.spec.destination,
        destination_fingerprint: &online.spec.destination_fingerprint,
        control_socket: &online.spec.control_socket,
        route: online.spec.route.kind(),
        route_profile: online.spec.route_profile.as_ref(),
    })
    .map_err(|_| ownership_conflict())?;
    static NEXT_RECEIPT: AtomicU64 = AtomicU64::new(1);
    let temporary = target_dir.join(format!(
        ".{RECEIPT_NAME}.{}.{}.{}.tmp",
        online.spec.generation,
        std::process::id(),
        NEXT_RECEIPT.fetch_add(1, Ordering::Relaxed),
    ));
    let final_path = target_dir.join(RECEIPT_NAME);
    let receipt =
        tokio::task::spawn_blocking(move || PreparedReceipt::write(temporary, final_path, bytes))
            .await
            .map_err(|_| ownership_conflict())??;
    Ok((online, receipt))
}

#[cfg(unix)]
pub fn run_sftp_channel(target_id: &str, generation: u64, ssh_args: &[String]) -> io::Result<()> {
    let span = tracing::info_span!(target:"polar_station_operation", "resource_sftp_prepare",
        component="Resources", target_id, generation, outcome="failed");
    let entered = span.enter();
    let normalized_args = normalize_sftp_channel_args(ssh_args)?;
    if target_id.is_empty()
        || target_id.len() > 64
        || !target_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid SSH target ID",
        ));
    }
    tracing::info!(target:"polar_station_operation", event="resource_sftp_arguments_validated");
    let runtime_root = platform_ssh_runtime_root();
    let target_dir = target_runtime_directory(&runtime_root, target_id);
    let receipt_path = target_dir.join(RECEIPT_NAME);
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let file = options.open(&receipt_path)?;
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no arguments and is thread-safe.
    let uid = unsafe { libc::geteuid() };
    if metadata.uid() != uid || !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "SSH ownership receipt is not private",
        ));
    }
    let receipt: OwnedReceipt = serde_json::from_reader(file)?;
    tracing::info!(target:"polar_station_operation", event="resource_sftp_receipt_read");
    if receipt.target_id != target_id
        || receipt.generation != generation
        || !socket_matches_receipt(
            &receipt.control_socket,
            &target_dir,
            generation,
            receipt.route,
        )
        || !receipt_configuration_fingerprint_matches(
            &receipt.destination,
            &receipt.destination_fingerprint,
            receipt.route,
            receipt.route_profile.as_ref(),
        )
        || !polar_tmux_protocol::is_canonical_ssh_destination(&receipt.destination)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "SSH ownership receipt does not match the requested generation",
        ));
    }
    tracing::info!(target:"polar_station_operation", event="resource_sftp_receipt_validated");
    verify_control_socket(&receipt.control_socket)?;
    // SAFETY: signal 0 performs a liveness/permission check and does not mutate
    // the process.
    if unsafe { libc::kill(receipt.master_pid as i32, 0) } != 0 {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "SSH master process is not available",
        ));
    }
    tracing::info!(target:"polar_station_operation", event="resource_sftp_socket_and_owner_validated");

    use std::os::unix::process::CommandExt;
    span.record("outcome", "ok");
    tracing::info!(target:"polar_station_operation", event="resource_sftp_exec_requested");
    drop(entered);
    drop(span);
    // exec replaces the process without invoking atexit. Seal the existing
    // bounded trace before handing the already-owned channel to OpenSSH.
    crate::operation_trace::finish_process_trace();
    let error = std::process::Command::new("/usr/bin/ssh")
        .args(["-S".as_ref(), receipt.control_socket.as_os_str()])
        .args([
            "-o",
            "ControlMaster=no",
            "-o",
            "ProxyCommand=/usr/bin/false",
        ])
        .arg(receipt.destination)
        .args(normalized_args)
        .exec();
    Err(error)
}

#[cfg(unix)]
fn normalize_sftp_channel_args(ssh_args: &[String]) -> io::Result<[String; 2]> {
    let direct = ssh_args == ["-s", "sftp"];
    let sshfs = ssh_args
        == [
            "-x",
            "-a",
            "-oClearAllForwardings=yes",
            "-2",
            "polar-resource",
            "-s",
            "sftp",
        ]
        || ssh_args
            == [
                "-x",
                "-a",
                "-oClearAllForwardings=yes",
                "-oServerAliveInterval=15",
                "-oServerAliveCountMax=2",
                "-2",
                "polar-resource",
                "-s",
                "sftp",
            ];
    if !direct && !sshfs {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "only the SFTP subsystem is permitted",
        ));
    }
    Ok(["-s".to_owned(), "sftp".to_owned()])
}

fn ownership_conflict() -> SshConnectionError {
    SshConnectionError::new(
        SshConnectionErrorCode::OwnershipConflict,
        "the private SSH runtime path is not safely owned by this Host user",
    )
}



// Producer and consumers share the exact socket identity grammar. Legacy names
// are accepted only for the same private directory, route and generation.
fn control_socket_name(generation: u64, route: &str, attempt: Option<u64>) -> String {
    match attempt {
        Some(attempt) => format!("g{generation}{route}-{attempt:x}.sock"),
        None => format!("g{generation}{route}.sock"),
    }
}

fn socket_matches_receipt(
    path: &Path,
    directory: &Path,
    generation: u64,
    route: SshRouteKind,
) -> bool {
    let tag = match route {
        SshRouteKind::Opaque => "o",
        SshRouteKind::Lan => "l",
        SshRouteKind::Wan => "w",
    };
    if path.parent() != Some(directory) {
        return false;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if name == control_socket_name(generation, tag, None) {
        return true;
    }
    let prefix = format!("g{generation}{tag}-");
    name.strip_prefix(&prefix)
        .and_then(|name| name.strip_suffix(".sock"))
        .filter(|value| !value.is_empty())
        .and_then(|value| u64::from_str_radix(value, 16).ok())
        .is_some_and(|attempt| name == control_socket_name(generation, tag, Some(attempt)))
}


