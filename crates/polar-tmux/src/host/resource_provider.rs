use std::{
    collections::BTreeMap,
    env,
    ffi::{OsStr, OsString},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::{fs, io::Write};

use polar_tmux_resources::{
    plan_resource_update, AndroidMirrorSpec, FileMappingSpec, PortForwardSpec, ResourceCatalog,
    ResourceId, ResourceRuntimeState, ResourceRuntimeStatus, ResourceSpec,
};
use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use tokio::net::TcpListener;
use tokio::{
    net::TcpStream,
    process::{Child, Command},
};

use crate::{
    ssh_connection_manager::{SshConnectionLease, SshConnectionManager},
    target_registry::TargetRegistry,
};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(8);
#[cfg(any(target_os = "macos", target_os = "linux"))]
const PROVIDER_START_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(any(target_os = "macos", target_os = "linux"))]
const PROVIDER_START_POLL: Duration = Duration::from_millis(100);
#[cfg(any(target_os = "macos", target_os = "linux"))]
const FILE_MAPPING_OWNERSHIP_RECEIPT: &str = "file-mapping-ownership.v1.json";
#[cfg(any(target_os = "macos", target_os = "linux"))]
const FILE_MAPPING_OWNERSHIP_SCHEMA_VERSION: u32 = 1;
#[cfg(any(target_os = "macos", target_os = "linux"))]
const ORPHAN_PROVIDER_STOP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForwardOperation {
    Start,
    Cancel,
}

impl ForwardOperation {
    
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForwardRule {
    bind_address: String,
    local_port: u16,
    remote_host: String,
    remote_port: u16,
}

impl ForwardRule {
    pub(crate) fn argument(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.bind_address, self.local_port, self.remote_host, self.remote_port
        )
    }

    fn probe_address(&self) -> Option<SocketAddr> {
        let ip = match self.bind_address.to_ascii_lowercase().as_str() {
            "localhost" | "0.0.0.0" => IpAddr::V4(Ipv4Addr::LOCALHOST),
            "::" => IpAddr::V6(Ipv6Addr::LOCALHOST),
            value => value.parse().ok()?,
        };
        Some(SocketAddr::new(ip, self.local_port))
    }
}

pub(crate) fn expand_forward_rules(spec: &PortForwardSpec) -> Vec<ForwardRule> {
    (spec.local_port_start..=spec.local_port_end)
        .map(|local_port| ForwardRule {
            bind_address: spec.bind_address.clone(),
            local_port,
            remote_host: spec.remote_host.clone(),
            remote_port: spec.remote_port_start + (local_port - spec.local_port_start),
        })
        .collect()
}

fn android_forward_rule(spec: &AndroidMirrorSpec) -> ForwardRule {
    ForwardRule {
        bind_address: spec.bind_address.clone(),
        local_port: spec.local_adb_port,
        remote_host: spec.remote_adb_host.clone(),
        remote_port: spec.remote_adb_port,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ResourceProviderError {
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
}

impl ResourceProviderError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }

    fn dependency() -> Self {
        Self::new(
            "resource_dependency_missing",
            "A required Resource provider executable is not installed.",
        )
    }

    fn ssh() -> Self {
        Self::new(
            "resource_ssh_unavailable",
            "The shared SSH connection is unavailable. Retry after it reconnects.",
        )
    }

    fn start() -> Self {
        Self::new(
            "resource_provider_start_failed",
            "The Resource provider could not start or could not be observed.",
        )
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn ownership() -> Self {
        Self::new(
            "resource_ownership_conflict",
            "The configured local path is not safely owned by Polar Station.",
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CleanupOutcome {
    Stopped,
    AlreadyGone,
}

struct PortReceipt {
    lease: SshConnectionLease,
    rules: Vec<ForwardRule>,
}

struct AndroidReceipt {
    spec: AndroidMirrorSpec,
    lease: SshConnectionLease,
    rule: ForwardRule,
    endpoint: String,
    scrcpy: Option<Child>,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
struct FileReceipt {
    lease: SshConnectionLease,
    child: Child,
    mount_path: PathBuf,
    ownership_receipt_path: PathBuf,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileMappingOwnershipReceiptV1 {
    schema_version: u32,
    resource_id: String,
    execution_target_id: String,
    remote_path: String,
    mount_path: String,
    provider_pid: u32,
    provider_executable: String,
    provider_started_at: String,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl FileMappingOwnershipReceiptV1 {
    pub(crate) fn for_running_provider(
        spec: &FileMappingSpec,
        provider_pid: u32,
        provider_executable: &str,
        provider_started_at: &str,
    ) -> Self {
        Self {
            schema_version: FILE_MAPPING_OWNERSHIP_SCHEMA_VERSION,
            resource_id: spec.id.to_string(),
            execution_target_id: spec.execution_target_id.clone(),
            remote_path: spec.remote_path.clone(),
            mount_path: spec.local_path.clone(),
            provider_pid,
            provider_executable: provider_executable.to_owned(),
            provider_started_at: provider_started_at.to_owned(),
        }
    }

    pub(crate) fn matches(&self, spec: &FileMappingSpec) -> bool {
        self.schema_version == FILE_MAPPING_OWNERSHIP_SCHEMA_VERSION
            && self.resource_id == spec.id.to_string()
            && self.execution_target_id == spec.execution_target_id
            && self.remote_path == spec.remote_path
            && self.mount_path == spec.local_path
            && self.provider_pid > 0
            && !self.provider_executable.is_empty()
            && !self.provider_started_at.is_empty()
    }
}

enum ResourceReceipt {
    Port(PortReceipt),
    Android(Box<AndroidReceipt>),
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    File(Box<FileReceipt>),
}

impl ResourceReceipt {
    fn generation(&self) -> u64 {
        match self {
            Self::Port(receipt) => receipt.lease.generation(),
            Self::Android(receipt) => receipt.lease.generation(),
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            Self::File(receipt) => receipt.lease.generation(),
        }
    }
}

pub(crate) struct SystemResourceProvider {
    targets: TargetRegistry,
    ssh_connections: SshConnectionManager,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    runtime_root: PathBuf,
    receipts: BTreeMap<ResourceId, ResourceReceipt>,
    failures: BTreeMap<ResourceId, ResourceProviderError>,
}

impl SystemResourceProvider {
    pub(crate) fn new(
        targets: TargetRegistry,
        ssh_connections: SshConnectionManager,
        runtime_root: PathBuf,
    ) -> Self {
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let _ = runtime_root;
        Self {
            targets,
            ssh_connections,
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            runtime_root,
            receipts: BTreeMap::new(),
            failures: BTreeMap::new(),
        }
    }

    pub(crate) fn ensure_connection(&self, target_id: &str) {
        if let Some(target) = self.targets.get(target_id) {
            if let Err(error) = target.ensure_resource_supervisor(&self.ssh_connections) {
                tracing::warn!(%error, "Resource SSH supervisor unavailable");
            }
        }
    }

    pub(crate) fn fork(&self) -> Self {
        Self::new(
            self.targets.clone(),
            self.ssh_connections.clone(),
            self.runtime_root.clone(),
        )
    }

    pub(crate) async fn update(
        &mut self,
        id: ResourceId,
        previous: Option<&ResourceSpec>,
        desired: Option<ResourceSpec>,
        generation: Option<u64>,
        restart: bool,
        mirror: bool,
    ) -> ResourceRuntimeStatus {
        let observed = self.receipts.get(&id).map(ResourceReceipt::generation);
        let plan = plan_resource_update(
            previous,
            desired.as_ref(),
            observed,
            generation,
            restart || (observed.is_some() && self.failures.contains_key(&id)),
        );
        self.failures.remove(&id);
        if plan.stop {
            if let Err(error) = self.stop(id).await {
                self.failures.insert(id, error);
            }
        }
        if plan.start && !self.receipts.contains_key(&id) && !self.failures.contains_key(&id) {
            match self
                .start(desired.clone().expect("enabled desired resource"))
                .await
            {
                Ok(receipt) => {
                    self.receipts.insert(id, receipt);
                }
                Err(error) => {
                    self.failures.insert(id, error);
                }
            }
        }
        if desired.as_ref().is_some_and(ResourceSpec::enabled)
            && generation.is_none()
            && !self.failures.contains_key(&id)
        {
            self.failures.insert(id, ResourceProviderError::ssh());
        }
        let catalog = single_catalog(desired.clone());
        if mirror && !self.failures.contains_key(&id) {
            if let Err(error) = self.open_mirror(&catalog, id).await {
                self.failures.insert(id, error);
            }
        }
        self.statuses(&catalog)
            .await
            .remove(&id)
            .unwrap_or_else(|| {
                if let Some(error) = self.failures.get(&id) {
                    ResourceRuntimeStatus {
                        state: ResourceRuntimeState::NeedsAttention,
                        generation: observed.unwrap_or(0),
                        message: error.message.into(),
                        can_restart: true,
                        can_open_mirror: false,
                    }
                } else {
                    ResourceRuntimeStatus::stopped()
                }
            })
    }

    pub(crate) async fn discard_obsolete(&mut self, id: ResourceId) {
        if let Err(error) = self.stop(id).await {
            self.failures.insert(id, error);
        }
    }

    /// Wait on the exact owned child. Dropping this future does not stop it.
    pub(crate) async fn child_exited(&mut self) {
        for receipt in self.receipts.values_mut() {
            match receipt {
                ResourceReceipt::Android(receipt) => {
                    if let Some(child) = receipt.scrcpy.as_mut() {
                        let _ = child.wait().await;
                        receipt.scrcpy = None;
                        return;
                    }
                }
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                ResourceReceipt::File(receipt) => {
                    let _ = receipt.child.wait().await;
                    return;
                }
                ResourceReceipt::Port(_) => {}
            }
        }
        std::future::pending::<()>().await;
    }

    pub(crate) async fn process_exit(&mut self) {
        self.refresh_processes().await;
    }

    pub(crate) async fn lease_current(&self) -> bool {
        for receipt in self.receipts.values() {
            let lease = match receipt {
                ResourceReceipt::Port(r) => &r.lease,
                ResourceReceipt::Android(r) => &r.lease,
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                ResourceReceipt::File(r) => &r.lease,
            };
            if self.ssh_connections.validate(lease).await.is_err() {
                return false;
            }
        }
        true
    }

    pub(crate) async fn open_mirror(
        &mut self,
        catalog: &ResourceCatalog,
        id: ResourceId,
    ) -> Result<BTreeMap<ResourceId, ResourceRuntimeStatus>, ResourceProviderError> {
        let executable =
            resolve_executable("scrcpy").ok_or_else(ResourceProviderError::dependency)?;
        let Some(ResourceReceipt::Android(receipt)) = self.receipts.get_mut(&id) else {
            return Err(ResourceProviderError::new(
                "android_mirror_not_running",
                "The Android tunnel must be running before opening the mirror.",
            ));
        };
        if let Some(child) = receipt.scrcpy.as_mut() {
            if child
                .try_wait()
                .map_err(|_| ResourceProviderError::start())?
                .is_none()
            {
                return Ok(self.statuses(catalog).await);
            }
        }
        receipt.scrcpy = Some(spawn_scrcpy(
            &executable,
            &receipt.endpoint,
            &receipt.spec.scrcpy_arguments,
        )?);
        observe_child_running(receipt.scrcpy.as_mut().expect("scrcpy child")).await?;
        Ok(self.statuses(catalog).await)
    }

    pub(crate) async fn statuses(
        &mut self,
        catalog: &ResourceCatalog,
    ) -> BTreeMap<ResourceId, ResourceRuntimeStatus> {
        let mut statuses = BTreeMap::new();
        for spec in all_specs(catalog) {
            let id = spec.id();
            let status = if let Some(error) = self.failures.get(&id) {
                ResourceRuntimeStatus {
                    state: failure_runtime_state(error),
                    generation: self
                        .receipts
                        .get(&id)
                        .map_or(0, ResourceReceipt::generation),
                    message: error.message.to_owned(),
                    can_restart: spec.enabled() && error.code != "resource_ssh_unavailable",
                    can_open_mirror: false,
                }
            } else if !spec.enabled() {
                ResourceRuntimeStatus::disabled()
            } else if let Some(receipt) = self.receipts.get(&id) {
                ResourceRuntimeStatus {
                    state: ResourceRuntimeState::Running,
                    generation: receipt.generation(),
                    message: running_message(receipt).to_owned(),
                    can_restart: true,
                    can_open_mirror: matches!(receipt, ResourceReceipt::Android(_)),
                }
            } else {
                ResourceRuntimeStatus::stopped()
            };
            statuses.insert(id, status);
        }
        statuses
    }

    async fn start(
        &mut self,
        spec: ResourceSpec,
    ) -> Result<ResourceReceipt, ResourceProviderError> {
        match spec {
            ResourceSpec::PortForward(spec) => self.start_port(spec).await,
            ResourceSpec::AndroidMirror(spec) => self.start_android(spec).await,
            ResourceSpec::FileMapping(spec) => self.start_file(spec).await,
        }
    }

    #[tracing::instrument(
        target = "polar_station_operation",
        name = "resource_start_port",
        skip_all,
        fields(component = "Resources", outcome = "unclassified")
    )]
    async fn start_port(
        &mut self,
        spec: PortForwardSpec,
    ) -> Result<ResourceReceipt, ResourceProviderError> {
        let lease = self.lease(&spec.execution_target_id).await?;
        let rules = expand_forward_rules(&spec);
        self.receipts.insert(
            spec.id,
            ResourceReceipt::Port(PortReceipt {
                lease: lease.clone(),
                rules: Vec::new(),
            }),
        );
        for rule in &rules {
            if let Err(error) = self
                .change_forward(&lease, ForwardOperation::Start, rule)
                .await
            {
                let _ = self.stop(spec.id).await;
                return Err(error);
            }
            if let Some(ResourceReceipt::Port(receipt)) = self.receipts.get_mut(&spec.id) {
                receipt.rules.push(rule.clone());
            }
        }
        for rule in &rules {
            if let Err(error) = observe_forward(rule).await {
                let _ = self.stop(spec.id).await;
                return Err(error);
            }
        }
        tracing::Span::current().record("outcome", "ok");
        Ok(self
            .receipts
            .remove(&spec.id)
            .expect("owned forward receipt"))
    }

    #[tracing::instrument(
        target = "polar_station_operation",
        name = "resource_start_android",
        skip_all,
        fields(component = "Resources", outcome = "unclassified")
    )]
    async fn start_android(
        &mut self,
        spec: AndroidMirrorSpec,
    ) -> Result<ResourceReceipt, ResourceProviderError> {
        let adb = resolve_executable("adb").ok_or_else(ResourceProviderError::dependency)?;
        let lease = self.lease(&spec.execution_target_id).await?;
        let rule = android_forward_rule(&spec);
        self.change_forward(&lease, ForwardOperation::Start, &rule)
            .await?;
        let endpoint = adb_endpoint(&spec.bind_address, spec.local_adb_port);
        self.receipts.insert(
            spec.id,
            ResourceReceipt::Android(Box::new(AndroidReceipt {
                spec: spec.clone(),
                lease,
                rule: rule.clone(),
                endpoint: endpoint.clone(),
                scrcpy: None,
            })),
        );
        let result = async {
            observe_forward(&rule).await?;
            let output =
                run_command_output(&adb, [OsString::from("connect"), endpoint.into()]).await?;
            let response = String::from_utf8_lossy(&output.stdout);
            if !output.status.success()
                || !(response.contains("connected to") || response.contains("already connected"))
            {
                return Err(ResourceProviderError::start());
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            let _ = self.stop(spec.id).await;
            return Err(error);
        }
        tracing::Span::current().record("outcome", "ok");
        Ok(self
            .receipts
            .remove(&spec.id)
            .expect("owned Android receipt"))
    }

    #[cfg(target_os = "macos")]
    #[tracing::instrument(
        target = "polar_station_operation",
        name = "resource_start_file",
        skip_all,
        fields(component = "Resources", outcome = "unclassified")
    )]
    async fn start_file(
        &self,
        spec: FileMappingSpec,
    ) -> Result<ResourceReceipt, ResourceProviderError> {
        let rclone = resolve_executable("rclone").ok_or_else(ResourceProviderError::dependency)?;
        let mount_webdav = PathBuf::from("/sbin/mount_webdav");
        if !mount_webdav.is_file() {
            return Err(ResourceProviderError::dependency());
        }
        let lease = self.lease(&spec.execution_target_id).await?;
        let resource_dir = self.prepare_resource_directory(spec.id)?;
        recover_owned_file_mapping(&spec, &resource_dir).await?;
        let mount_path = prepare_mount_path(&spec.local_path)?;
        if is_mounted(&mount_path).await {
            return Err(ResourceProviderError::ownership());
        }
        let cache_dir = resource_dir.join("cache");
        create_private_directory(&cache_dir)?;
        let log_path = resource_dir.join("rclone.log");
        let port = reserve_loopback_port().await?;
        let current_exe = env::current_exe().map_err(|_| ResourceProviderError::start())?;
        let sftp_executable = prepare_sftp_executable(&resource_dir, &current_exe)?;
        let sftp_command = format!(
            "{} sftp-channel --target-id {} --generation {}",
            sftp_executable.display(),
            lease.target_id(),
            lease.generation()
        );
        let mut command = Command::new(&rclone);
        command
            .arg("serve")
            .arg("webdav")
            .arg(format!(":sftp:{}", spec.remote_path))
            .args(["--addr", &format!("127.0.0.1:{port}")])
            .args(["--sftp-ssh", &sftp_command])
            .args(["--sftp-connections", "3"])
            .arg("--sftp-disable-hashcheck")
            .args(["--transfers", "2"])
            .args(["--vfs-cache-mode", "full"])
            .args(["--vfs-write-back", "1s"])
            .args(["--vfs-cache-max-size", "10G"])
            .args(["--vfs-cache-max-age", "1h"])
            .args(["--dir-cache-time", "10s"])
            .args(["--poll-interval", "0"])
            .arg("--cache-dir")
            .arg(cache_dir)
            .arg("--log-file")
            .arg(log_path)
            .args(["--log-level", "INFO"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        super::child_process::configure_parent_lease(&mut command)
            .map_err(|_| ResourceProviderError::start())?;
        let mut child = command
            .spawn()
            .map_err(|_| ResourceProviderError::start())?;
        let ownership_receipt_path =
            record_running_file_provider(&spec, &resource_dir, &rclone, &child).await?;
        if let Err(error) = observe_listener_and_child(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            &mut child,
        )
        .await
        {
            let _ = child.kill().await;
            let _ = remove_ownership_receipt(&ownership_receipt_path);
            return Err(error);
        }
        let url = format!("http://127.0.0.1:{port}/");
        let volume = format!("polar-{}", &spec.id.to_string()[..8]);
        let output = run_command_output(
            &mount_webdav,
            [
                OsString::from("-S"),
                OsString::from("-v"),
                volume.into(),
                url.into(),
                mount_path.as_os_str().to_owned(),
            ],
        )
        .await?;
        if !output.status.success() || !is_mounted(&mount_path).await {
            let _ = child.kill().await;
            let _ = remove_ownership_receipt(&ownership_receipt_path);
            return Err(ResourceProviderError::start());
        }
        Ok(ResourceReceipt::File(Box::new(FileReceipt {
            lease,
            child,
            mount_path,
            ownership_receipt_path,
        })))
    }

    #[cfg(target_os = "linux")]
    #[tracing::instrument(
        target = "polar_station_operation",
        name = "resource_start_file",
        skip_all,
        fields(component = "Resources", outcome = "unclassified")
    )]
    async fn start_file(
        &self,
        spec: FileMappingSpec,
    ) -> Result<ResourceReceipt, ResourceProviderError> {
        let sshfs = resolve_executable("sshfs").ok_or_else(ResourceProviderError::dependency)?;
        let lease = self.lease(&spec.execution_target_id).await?;
        let resource_dir = self.prepare_resource_directory(spec.id)?;
        recover_owned_file_mapping(&spec, &resource_dir).await?;
        let mount_path = prepare_mount_path(&spec.local_path)?;
        if is_mounted(&mount_path).await {
            return Err(ResourceProviderError::ownership());
        }
        let current_exe = env::current_exe().map_err(|_| ResourceProviderError::start())?;
        let sftp_executable = prepare_sftp_executable(&resource_dir, &current_exe)?;
        let arguments = linux_sshfs_arguments(
            &sftp_executable,
            lease.target_id(),
            lease.generation(),
            &spec.remote_path,
            &mount_path,
        );
        let mut command = Command::new(&sshfs);
        command
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        super::child_process::configure_parent_lease(&mut command)
            .map_err(|_| ResourceProviderError::start())?;
        let mut child = command
            .spawn()
            .map_err(|_| ResourceProviderError::start())?;
        let ownership_receipt_path =
            record_running_file_provider(&spec, &resource_dir, &sshfs, &child).await?;
        if let Err(error) = observe_mount_and_child(&mount_path, &mut child).await {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let _ = remove_ownership_receipt(&ownership_receipt_path);
            return Err(error);
        }
        Ok(ResourceReceipt::File(Box::new(FileReceipt {
            lease,
            child,
            mount_path,
            ownership_receipt_path,
        })))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    #[tracing::instrument(
        target = "polar_station_operation",
        name = "resource_start_file",
        skip_all,
        fields(component = "Resources", outcome = "unclassified")
    )]
    async fn start_file(
        &self,
        _spec: FileMappingSpec,
    ) -> Result<ResourceReceipt, ResourceProviderError> {
        Err(ResourceProviderError::new(
            "resource_file_mapping_platform_pending",
            "File Mapping is not available on this platform build.",
        ))
    }

    #[tracing::instrument(
        target = "polar_station_operation",
        name = "resource_stop",
        skip_all,
        fields(component = "Resources", outcome = "unclassified")
    )]
    async fn stop(&mut self, id: ResourceId) -> Result<CleanupOutcome, ResourceProviderError> {
        self.failures.remove(&id);
        let Some(mut receipt) = self.receipts.remove(&id) else {
            return Ok(CleanupOutcome::AlreadyGone);
        };
        let already_gone = match &receipt {
            ResourceReceipt::Port(r) => r.lease.master_exited(),
            ResourceReceipt::Android(r) => r.lease.master_exited() && r.scrcpy.is_none(),
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            ResourceReceipt::File(_) => false,
        };
        let result = match &mut receipt {
            ResourceReceipt::Port(receipt) => {
                let mut first_error = None;
                let mut retained = Vec::new();
                for rule in receipt.rules.drain(..).rev() {
                    if let Err(error) = self
                        .change_forward(&receipt.lease, ForwardOperation::Cancel, &rule)
                        .await
                    {
                        first_error.get_or_insert(error);
                        retained.push(rule);
                    }
                }
                retained.reverse();
                receipt.rules = retained;
                if let Some(error) = first_error {
                    Err(error)
                } else {
                    Ok(())
                }
            }
            ResourceReceipt::Android(receipt) => {
                if let Some(mut child) = receipt.scrcpy.take() {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                }
                if let Some(adb) = resolve_executable("adb") {
                    let _ = run_command_output(
                        &adb,
                        [
                            OsString::from("disconnect"),
                            receipt.endpoint.clone().into(),
                        ],
                    )
                    .await;
                }
                self.change_forward(&receipt.lease, ForwardOperation::Cancel, &receipt.rule)
                    .await
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            ResourceReceipt::File(receipt) => {
                let unmount_result = unmount_file_mapping(&receipt.mount_path).await;
                if unmount_result.is_ok() {
                    let _ = receipt.child.kill().await;
                    let _ = receipt.child.wait().await;
                    remove_ownership_receipt(&receipt.ownership_receipt_path)?;
                }
                unmount_result
            }
        };
        if let Err(error) = result {
            tracing::Span::current().record("outcome", "blocked");
            self.receipts.insert(id, receipt);
            return Err(error);
        }
        tracing::Span::current().record("outcome", "ok");
        Ok(if already_gone {
            CleanupOutcome::AlreadyGone
        } else {
            CleanupOutcome::Stopped
        })
    }

    #[tracing::instrument(
        target = "polar_station_operation",
        name = "resource_lease",
        skip_all,
        fields(component = "Resources", outcome = "unclassified")
    )]
    async fn lease(&self, target_id: &str) -> Result<SshConnectionLease, ResourceProviderError> {
        let target = self
            .targets
            .get(target_id)
            .ok_or_else(ResourceProviderError::ssh)?;
        target
            .resource_ssh_lease(&self.ssh_connections)
            .await
            .map_err(|_| ResourceProviderError::ssh())?
            .ok_or_else(ResourceProviderError::ssh)
    }

    #[tracing::instrument(
        target = "polar_station_operation",
        name = "resource_change_forward",
        skip_all,
        fields(component = "Resources", outcome = "unclassified")
    )]
    async fn change_forward(
        &self,
        lease: &SshConnectionLease,
        operation: ForwardOperation,
        rule: &ForwardRule,
    ) -> Result<(), ResourceProviderError> {
        // A replacement route generation makes the previous lease stale in
        // the manager before consumers receive the event. Cancellation is
        // nevertheless safe and necessary during the bounded old-master
        // grace window: the receipt owns a previously verified private
        // ControlPath, and ProxyCommand remains fail-closed. New forwards
        // still require a current manager lease.
        if operation == ForwardOperation::Start {
            self.ssh_connections
                .validate(lease)
                .await
                .map_err(|_| ResourceProviderError::ssh())?;
        }
        if operation == ForwardOperation::Cancel && lease.master_exited() {
            tracing::info!(target:"polar_station_operation", component="Resources", event="resource_cleanup_already_gone");
            return Ok(());
        }
        let output = run_command_output(
            Path::new("/usr/bin/ssh"),
            lease.forward_control_args(operation == ForwardOperation::Cancel, &rule.argument()),
        )
        .await;
        if output.as_ref().is_ok_and(|out| out.status.success()) {
            return Ok(());
        }
        if operation == ForwardOperation::Cancel {
            // Retirement grace is owned by SSH. Await its actual wait/reap fact,
            // bounded by the existing command timeout, rather than guessing PID state.
            if tokio::time::timeout(COMMAND_TIMEOUT, lease.wait_for_exit()).await == Ok(true) {
                tracing::info!(target:"polar_station_operation", component="Resources", event="resource_cleanup_already_gone");
                return Ok(());
            }
        }
        Err(output.err().unwrap_or_else(ResourceProviderError::start))
    }

    async fn refresh_processes(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let mut failed = Vec::new();
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        for (id, receipt) in &mut self.receipts {
            match receipt {
                ResourceReceipt::Android(receipt) => {
                    if let Some(child) = receipt.scrcpy.as_mut() {
                        if child.try_wait().ok().flatten().is_some() {
                            receipt.scrcpy = None;
                        }
                    }
                }
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                ResourceReceipt::File(receipt) => {
                    if receipt.child.try_wait().ok().flatten().is_some()
                        || !is_mounted(&receipt.mount_path).await
                    {
                        failed.push(*id);
                    }
                }
                ResourceReceipt::Port(_) => {}
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        for receipt in self.receipts.values_mut() {
            if let ResourceReceipt::Android(receipt) = receipt {
                if let Some(child) = receipt.scrcpy.as_mut() {
                    if child.try_wait().ok().flatten().is_some() {
                        receipt.scrcpy = None;
                    }
                }
            }
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        for id in failed {
            let error = match self.stop(id).await {
                Ok(_) => ResourceProviderError::start(),
                Err(error) => error,
            };
            self.failures.insert(id, error);
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn prepare_resource_directory(&self, id: ResourceId) -> Result<PathBuf, ResourceProviderError> {
        create_private_directory(&self.runtime_root)?;
        let path = self.runtime_root.join(id.to_string());
        create_private_directory(&path)?;
        Ok(path)
    }
}

// Async shutdown belongs to ResourceRecovery. Drop never blocks the runtime
// on ssh/adb/unmount, and keeps persistent mount ownership for later recovery.
impl Drop for SystemResourceProvider {
    fn drop(&mut self) {
        for receipt in self.receipts.values_mut() {
            match receipt {
                ResourceReceipt::Android(receipt) => {
                    if let Some(child) = &mut receipt.scrcpy {
                        let _ = child.start_kill();
                    }
                }
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                ResourceReceipt::File(receipt) => {
                    let _ = receipt.child.start_kill();
                }
                ResourceReceipt::Port(_) => {}
            }
        }
    }
}

pub(crate) fn platform_resource_runtime_root() -> PathBuf {
    if let Some(runtime_root) = env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime_root)
            .join("polar-tmux")
            .join("resources");
    }
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no arguments and is thread-safe.
        let uid = unsafe { libc::geteuid() };
        return env::temp_dir()
            .join(format!("polar-tmux-{uid}"))
            .join("resources");
    }
    #[allow(unreachable_code)]
    env::temp_dir().join("polar-tmux").join("resources")
}

pub(crate) fn single_catalog(spec: Option<ResourceSpec>) -> ResourceCatalog {
    let mut catalog = ResourceCatalog::empty();
    match spec {
        Some(ResourceSpec::FileMapping(spec)) => catalog.file_mappings.push(spec),
        Some(ResourceSpec::PortForward(spec)) => catalog.port_forwards.push(spec),
        Some(ResourceSpec::AndroidMirror(spec)) => catalog.android_mirrors.push(spec),
        None => {}
    }
    catalog
}

fn all_specs(catalog: &ResourceCatalog) -> Vec<ResourceSpec> {
    catalog
        .file_mappings
        .iter()
        .cloned()
        .map(ResourceSpec::FileMapping)
        .chain(
            catalog
                .port_forwards
                .iter()
                .cloned()
                .map(ResourceSpec::PortForward),
        )
        .chain(
            catalog
                .android_mirrors
                .iter()
                .cloned()
                .map(ResourceSpec::AndroidMirror),
        )
        .collect()
}



fn running_message(receipt: &ResourceReceipt) -> &'static str {
    match receipt {
        ResourceReceipt::Port(_) => "SSH forwarding is running.",
        ResourceReceipt::Android(receipt) if receipt.scrcpy.is_some() => {
            "ADB forwarding and scrcpy are running."
        }
        ResourceReceipt::Android(_) => "ADB forwarding is running. The mirror can be reopened.",
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        ResourceReceipt::File(_) => "Remote files are mounted.",
    }
}

fn failure_runtime_state(error: &ResourceProviderError) -> ResourceRuntimeState {
    if error.code == "resource_ssh_unavailable" {
        ResourceRuntimeState::WaitingForSsh
    } else {
        ResourceRuntimeState::NeedsAttention
    }
}

#[tracing::instrument(
    target = "polar_station_operation",
    name = "resource_run_command_output",
    skip_all,
    fields(component = "Resources", outcome = "unclassified")
)]
async fn run_command_output<I, S>(
    executable: impl AsRef<OsStr>,
    arguments: I,
) -> Result<std::process::Output, ResourceProviderError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let result = tokio::time::timeout(
        COMMAND_TIMEOUT,
        Command::new(executable)
            .args(arguments)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await;
    match result {
        Ok(Ok(output)) => {
            tracing::Span::current().record(
                "outcome",
                if output.status.success() {
                    "ok"
                } else {
                    "failed"
                },
            );
            tracing::info!(target:"polar_station_operation", component="Resources", event="resource_command_exit", exit_code=output.status.code().unwrap_or(-1));
            Ok(output)
        }
        Ok(Err(error)) => {
            tracing::Span::current().record("outcome", "failed");
            tracing::info!(target:"polar_station_operation", component="Resources", event="resource_command_error", errno=error.raw_os_error().unwrap_or(0));
            Err(ResourceProviderError::start())
        }
        Err(_) => {
            tracing::Span::current().record("outcome", "timeout");
            Err(ResourceProviderError::new(
                "resource_command_timeout",
                "Resource command timed out.",
            ))
        }
    }
}

#[tracing::instrument(
    target = "polar_station_operation",
    name = "resource_observe_forward",
    skip_all,
    fields(component = "Resources", outcome = "unclassified")
)]
async fn observe_forward(rule: &ForwardRule) -> Result<(), ResourceProviderError> {
    let Some(address) = rule.probe_address() else {
        return Ok(());
    };
    tokio::time::timeout(COMMAND_TIMEOUT, TcpStream::connect(address))
        .await
        .map_err(|_| ResourceProviderError::start())?
        .map(|_| ())
        .map_err(|_| ResourceProviderError::start())
}

fn adb_endpoint(bind_address: &str, port: u16) -> String {
    let address = match bind_address.to_ascii_lowercase().as_str() {
        "localhost" | "0.0.0.0" => "127.0.0.1".to_owned(),
        "::" => "::1".to_owned(),
        other => other.to_owned(),
    };
    if address.contains(':') {
        format!("[{address}]:{port}")
    } else {
        format!("{address}:{port}")
    }
}

fn spawn_scrcpy(
    executable: &Path,
    endpoint: &str,
    arguments: &[String],
) -> Result<Child, ResourceProviderError> {
    Command::new(executable)
        .args(["--serial", endpoint])
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| ResourceProviderError::start())
}

async fn observe_child_running(child: &mut Child) -> Result<(), ResourceProviderError> {
    tokio::time::sleep(Duration::from_millis(250)).await;
    match child
        .try_wait()
        .map_err(|_| ResourceProviderError::start())?
    {
        None => Ok(()),
        Some(_) => Err(ResourceProviderError::start()),
    }
}

fn resolve_executable(name: &str) -> Option<PathBuf> {
    let candidates: &[&str] = match name {
        "rclone" => &[
            "/opt/homebrew/bin/rclone",
            "/usr/local/bin/rclone",
            "/usr/bin/rclone",
        ],
        "adb" => &[
            "/opt/homebrew/bin/adb",
            "/usr/local/bin/adb",
            "/usr/bin/adb",
        ],
        "scrcpy" => &[
            "/opt/homebrew/bin/scrcpy",
            "/usr/local/bin/scrcpy",
            "/usr/bin/scrcpy",
        ],
        "sshfs" => &["/usr/bin/sshfs"],
        "fusermount3" => &["/usr/bin/fusermount3", "/bin/fusermount3"],
        _ => &[],
    };
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|candidate| candidate.is_file())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn create_private_directory(path: &Path) -> Result<(), ResourceProviderError> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ResourceProviderError::ownership());
        }
    } else {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder
            .create(path)
            .map_err(|_| ResourceProviderError::ownership())?;
    }
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| ResourceProviderError::ownership())?;
        let metadata =
            fs::symlink_metadata(path).map_err(|_| ResourceProviderError::ownership())?;
        // SAFETY: geteuid has no arguments and is thread-safe.
        let uid = unsafe { libc::geteuid() };
        if metadata.uid() != uid
            || metadata.permissions().mode() & 0o077 != 0
            || metadata.file_type().is_symlink()
            || !metadata.is_dir()
        {
            return Err(ResourceProviderError::ownership());
        }
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn prepare_sftp_executable(
    resource_dir: &Path,
    current_exe: &Path,
) -> Result<PathBuf, ResourceProviderError> {
    // rclone/sshfs parse their external command as a whitespace-delimited
    // string and do not honor shell quoting. Keep the actual helper path
    // behind a private, no-space symlink so app bundle paths remain usable.
    let conflict = || {
        ResourceProviderError::new(
            "resource_sftp_launcher_conflict",
            "The private SFTP helper link cannot be safely updated for this Host installation.",
        )
    };
    // Relocating the signed Host is legitimate. Only replace our private link,
    // never its target, an unexpected regular file, or another user's entry.
    let directory = fs::symlink_metadata(resource_dir).map_err(|_| conflict())?;
    // SAFETY: geteuid has no arguments and is thread-safe.
    let uid = unsafe { libc::geteuid() };
    if !directory.is_dir()
        || directory.file_type().is_symlink()
        || directory.uid() != uid
        || directory.permissions().mode() & 0o077 != 0
    {
        return Err(conflict());
    }
    let launcher = resource_dir.join("sftp-channel-host");
    match fs::symlink_metadata(&launcher) {
        Ok(metadata) => {
            if !metadata.file_type().is_symlink() || metadata.uid() != uid {
                return Err(conflict());
            }
            if fs::read_link(&launcher).map_err(|_| conflict())? != current_exe {
                let staged =
                    resource_dir.join(format!(".sftp-channel-host-{}", std::process::id()));
                std::os::unix::fs::symlink(current_exe, &staged).map_err(|_| conflict())?;
                if fs::rename(&staged, &launcher).is_err() {
                    let _ = fs::remove_file(&staged);
                    return Err(conflict());
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::os::unix::fs::symlink(current_exe, &launcher).map_err(|_| conflict())?;
        }
        Err(_) => return Err(conflict()),
    }
    Ok(launcher)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn ownership_receipt_path(resource_dir: &Path) -> PathBuf {
    resource_dir.join(FILE_MAPPING_OWNERSHIP_RECEIPT)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tracing::instrument(
    target = "polar_station_operation",
    name = "resource_record_running_file_provider",
    skip_all,
    fields(component = "Resources", outcome = "unclassified")
)]
async fn record_running_file_provider(
    spec: &FileMappingSpec,
    resource_dir: &Path,
    provider_executable: &Path,
    child: &Child,
) -> Result<PathBuf, ResourceProviderError> {
    let provider_pid = child.id().ok_or_else(ResourceProviderError::start)?;
    let provider_started_at = process_start_marker(provider_pid)
        .await
        .ok_or_else(ResourceProviderError::start)?;
    let receipt = FileMappingOwnershipReceiptV1::for_running_provider(
        spec,
        provider_pid,
        &provider_executable.to_string_lossy(),
        &provider_started_at,
    );
    let path = ownership_receipt_path(resource_dir);
    write_ownership_receipt(&path, &receipt)?;
    Ok(path)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn write_ownership_receipt(
    path: &Path,
    receipt: &FileMappingOwnershipReceiptV1,
) -> Result<(), ResourceProviderError> {
    let temporary = path.with_extension(format!(
        "json.tmp-{}-{}",
        std::process::id(),
        receipt.provider_pid
    ));
    if fs::symlink_metadata(&temporary).is_ok() {
        return Err(ResourceProviderError::ownership());
    }
    let mut bytes =
        serde_json::to_vec_pretty(receipt).map_err(|_| ResourceProviderError::ownership())?;
    bytes.push(b'\n');
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options
        .open(&temporary)
        .map_err(|_| ResourceProviderError::ownership())?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| ResourceProviderError::ownership())?;
    fs::rename(&temporary, path).map_err(|_| ResourceProviderError::ownership())?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|_| ResourceProviderError::ownership())?;
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn read_ownership_receipt(
    path: &Path,
) -> Result<Option<FileMappingOwnershipReceiptV1>, ResourceProviderError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ResourceProviderError::ownership()),
    };
    // SAFETY: geteuid has no arguments and is thread-safe.
    let uid = unsafe { libc::geteuid() };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != uid
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(ResourceProviderError::ownership());
    }
    let bytes = fs::read(path).map_err(|_| ResourceProviderError::ownership())?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| ResourceProviderError::ownership())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn remove_ownership_receipt(path: &Path) -> Result<(), ResourceProviderError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ResourceProviderError::ownership()),
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn ps_executable() -> &'static Path {
    if Path::new("/bin/ps").is_file() {
        Path::new("/bin/ps")
    } else {
        Path::new("/usr/bin/ps")
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn process_field(pid: u32, field: &str) -> Option<String> {
    let output = tokio::time::timeout(
        COMMAND_TIMEOUT,
        Command::new(ps_executable())
            .args(["-p", &pid.to_string(), "-o", field])
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!value.is_empty()).then_some(value)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn process_start_marker(pid: u32) -> Option<String> {
    process_field(pid, "lstart=").await
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn provider_process_matches(receipt: &FileMappingOwnershipReceiptV1) -> bool {
    let Some(started_at) = process_start_marker(receipt.provider_pid).await else {
        return false;
    };
    let Some(command) = process_field(receipt.provider_pid, "command=").await else {
        return false;
    };
    started_at == receipt.provider_started_at
        && command.starts_with(&receipt.provider_executable)
        && command.contains(&receipt.resource_id)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn stop_verified_orphan_provider(
    receipt: &FileMappingOwnershipReceiptV1,
) -> Result<(), ResourceProviderError> {
    if !provider_process_matches(receipt).await {
        return Ok(());
    }
    let pid = receipt.provider_pid as libc::pid_t;
    // SAFETY: the PID and process birth marker were verified against a private
    // ownership receipt immediately before signalling the exact provider.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        return Err(ResourceProviderError::ownership());
    }
    let deadline = tokio::time::Instant::now() + ORPHAN_PROVIDER_STOP_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if !provider_process_matches(receipt).await {
            return Ok(());
        }
        tokio::time::sleep(PROVIDER_START_POLL).await;
    }
    if !provider_process_matches(receipt).await {
        return Ok(());
    }
    // SAFETY: the exact process identity is revalidated after the TERM budget.
    if unsafe { libc::kill(pid, libc::SIGKILL) } != 0 {
        return Err(ResourceProviderError::ownership());
    }
    let deadline = tokio::time::Instant::now() + ORPHAN_PROVIDER_STOP_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if !provider_process_matches(receipt).await {
            return Ok(());
        }
        tokio::time::sleep(PROVIDER_START_POLL).await;
    }
    Err(ResourceProviderError::ownership())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn recover_owned_file_mapping(
    spec: &FileMappingSpec,
    resource_dir: &Path,
) -> Result<(), ResourceProviderError> {
    let receipt_path = ownership_receipt_path(resource_dir);
    let Some(receipt) = read_ownership_receipt(&receipt_path)? else {
        return Ok(());
    };
    if !receipt.matches(spec) {
        return Err(ResourceProviderError::ownership());
    }
    let mount_path = configured_mount_path(&receipt.mount_path)?;
    if is_mounted(&mount_path).await {
        unmount_file_mapping(&mount_path).await?;
    }
    stop_verified_orphan_provider(&receipt).await?;
    remove_ownership_receipt(&receipt_path)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn configured_mount_path(configured: &str) -> Result<PathBuf, ResourceProviderError> {
    let path = if let Some(relative) = configured.strip_prefix("~/") {
        let user_home = env::var_os("HOME").ok_or_else(ResourceProviderError::ownership)?;
        PathBuf::from(user_home).join(relative)
    } else {
        PathBuf::from(configured)
    };
    Ok(path)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn prepare_mount_path(configured: &str) -> Result<PathBuf, ResourceProviderError> {
    let path = configured_mount_path(configured)?;
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ResourceProviderError::ownership());
        }
        if fs::read_dir(&path)
            .map_err(|_| ResourceProviderError::ownership())?
            .next()
            .is_some()
        {
            return Err(ResourceProviderError::ownership());
        }
    } else {
        fs::create_dir_all(&path).map_err(|_| ResourceProviderError::ownership())?;
    }
    Ok(path)
}

#[cfg(target_os = "macos")]
async fn reserve_loopback_port() -> Result<u16, ResourceProviderError> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|_| ResourceProviderError::start())?;
    let port = listener
        .local_addr()
        .map_err(|_| ResourceProviderError::start())?
        .port();
    drop(listener);
    Ok(port)
}

#[cfg(target_os = "macos")]
#[tracing::instrument(
    target = "polar_station_operation",
    name = "resource_observe_listener_and_child",
    skip_all,
    fields(component = "Resources", outcome = "unclassified")
)]
async fn observe_listener_and_child(
    address: SocketAddr,
    child: &mut Child,
) -> Result<(), ResourceProviderError> {
    let deadline = tokio::time::Instant::now() + PROVIDER_START_TIMEOUT;
    loop {
        if child
            .try_wait()
            .map_err(|_| ResourceProviderError::start())?
            .is_some()
        {
            return Err(ResourceProviderError::start());
        }
        if TcpStream::connect(address).await.is_ok() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ResourceProviderError::start());
        }
        tokio::time::sleep(PROVIDER_START_POLL).await;
    }
}

#[cfg(target_os = "macos")]
async fn is_mounted(path: &Path) -> bool {
    is_mounted_blocking(path)
}

#[cfg(target_os = "linux")]
async fn is_mounted(path: &Path) -> bool {
    is_mounted_blocking(path)
}

#[cfg(target_os = "linux")]
fn is_mounted_blocking(path: &Path) -> bool {
    let Ok(contents) = fs::read_to_string("/proc/self/mountinfo") else {
        return false;
    };
    let expected = path.to_string_lossy().replace(' ', "\\040");
    contents.lines().any(|line| {
        line.split_ascii_whitespace()
            .nth(4)
            .is_some_and(|mountpoint| mountpoint == expected)
    })
}

#[cfg(target_os = "macos")]
#[tracing::instrument(
    target = "polar_station_operation",
    name = "resource_unmount_file_mapping",
    skip_all,
    fields(component = "Resources", outcome = "unclassified")
)]
async fn unmount_file_mapping(path: &Path) -> Result<(), ResourceProviderError> {
    if !is_mounted(path).await {
        return Ok(());
    }
    let _output =
        run_command_output(Path::new("/sbin/umount"), [path.as_os_str().to_owned()]).await?;
    // The provider can unmount concurrently as its SSH channel exits.
    // Command failure is diagnostic; the final mount observation decides.
    if !is_mounted(path).await {
        Ok(())
    } else {
        Err(ResourceProviderError::ownership())
    }
}

#[cfg(target_os = "linux")]
#[tracing::instrument(
    target = "polar_station_operation",
    name = "resource_unmount_file_mapping",
    skip_all,
    fields(component = "Resources", outcome = "unclassified")
)]
async fn unmount_file_mapping(path: &Path) -> Result<(), ResourceProviderError> {
    if !is_mounted(path).await {
        return Ok(());
    }
    let executable =
        resolve_executable("fusermount3").ok_or_else(ResourceProviderError::dependency)?;
    let _output = run_command_output(
        executable,
        [
            OsString::from("-u"),
            OsString::from("--"),
            path.as_os_str().to_owned(),
        ],
    )
    .await?;
    if !is_mounted(path).await {
        Ok(())
    } else {
        Err(ResourceProviderError::ownership())
    }
}

#[cfg(target_os = "linux")]
fn linux_sshfs_arguments(
    current_exe: &Path,
    target_id: &str,
    generation: u64,
    remote_path: &str,
    mount_path: &Path,
) -> Vec<OsString> {
    let ssh_command = format!(
        "{} sftp-channel --target-id {} --generation {}",
        current_exe.display(),
        target_id,
        generation
    );
    vec![
        format!("polar-resource:{remote_path}").into(),
        mount_path.as_os_str().to_owned(),
        OsString::from("-f"),
        OsString::from("-o"),
        format!("ssh_command={ssh_command}").into(),
        OsString::from("-o"),
        OsString::from("max_conns=1"),
        OsString::from("-o"),
        OsString::from("ServerAliveInterval=15"),
        OsString::from("-o"),
        OsString::from("ServerAliveCountMax=2"),
    ]
}

#[cfg(target_os = "linux")]
#[tracing::instrument(
    target = "polar_station_operation",
    name = "resource_observe_mount_and_child",
    skip_all,
    fields(component = "Resources", outcome = "unclassified")
)]
async fn observe_mount_and_child(
    mount_path: &Path,
    child: &mut Child,
) -> Result<(), ResourceProviderError> {
    let deadline = tokio::time::Instant::now() + PROVIDER_START_TIMEOUT;
    loop {
        if child
            .try_wait()
            .map_err(|_| ResourceProviderError::start())?
            .is_some()
        {
            return Err(ResourceProviderError::start());
        }
        if is_mounted(mount_path).await {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ResourceProviderError::start());
        }
        tokio::time::sleep(PROVIDER_START_POLL).await;
    }
}

#[cfg(target_os = "macos")]
fn is_mounted_blocking(path: &Path) -> bool {
    let Ok(output) = std::process::Command::new("/sbin/mount").output() else {
        return false;
    };
    let marker = format!(" on {} (", path.display());
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.contains(&marker))
}




