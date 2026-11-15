mod connections;
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex as StdMutex, RwLock as StdRwLock,
    },
};

use anyhow::{bail, Context, Result};
use polar_tmux_core::{TmuxClient, TmuxControlConnection, TmuxTarget};
use polar_tmux_protocol::{
    is_canonical_ssh_destination, is_canonical_tmux_socket_name, ExecutionTarget,
    ExecutionTargetDescriptor, ExecutionTargetKind, SshRouteProfileV1, SshTransportGenerationV1,
    TmuxTopologySnapshot,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{watch, RwLock};

use crate::ssh_connection_manager::{
    DohProvider, SshConnectionError, SshConnectionLease, SshConnectionManager, SshRouteProfile,
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

pub const LOCAL_TARGET_ID: &str = "local";

#[derive(Debug, Default)]
enum TopologyObservation {
    #[default]
    Uninitialized,
    Known(TmuxTopologySnapshot),
    UnknownAfterMutation,
}

#[derive(Debug, Clone)]
pub struct RegisteredTarget {
    id: String,
    display_name: String,
    kind: ExecutionTargetKind,
    client: TmuxClient,
    route_profile: Option<SshRouteProfile>,
    revision: Arc<AtomicU64>,
    configuration_revision: u64,
    operation_gate: Arc<RwLock<()>>,
    observed_topology: Arc<StdMutex<TopologyObservation>>,
    control: watch::Sender<Option<TargetControlConnection>>,
}

type TargetControlConnection = (Option<SshTransportGenerationV1>, TmuxControlConnection);

pub(crate) struct ManagedAttachClient {
    pub(crate) client: TmuxClient,
    pub(crate) transport_generation: Option<SshTransportGenerationV1>,
}

impl RegisteredTarget {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn ssh_destination(&self) -> Option<&str> {
        match self.client.target() {
            TmuxTarget::Ssh { destination } => Some(destination),
            TmuxTarget::Local | TmuxTarget::Wsl { .. } | TmuxTarget::MultiplexedSsh { .. } => None,
        }
    }

    pub async fn managed_client(
        &self,
        ssh_connections: &SshConnectionManager,
    ) -> Result<TmuxClient, SshConnectionError> {
        match self.client.target() {
            TmuxTarget::Ssh { destination } => {
                self.ensure_supervisor(ssh_connections, destination)?;
                let lease = self
                    .current_or_acquire_lease(ssh_connections, destination)
                    .await?;
                Ok(self.command_client(
                    self.client.clone().with_target(lease.tmux_target()?),
                    Some(&ssh_connections.transport_generation_for_lease(&lease)),
                ))
            }
            TmuxTarget::Local | TmuxTarget::Wsl { .. } | TmuxTarget::MultiplexedSsh { .. } => {
                Ok(self.command_client(self.client.clone(), None))
            }
        }
    }

    pub(crate) fn install_control_connection(
        &self,
        transport: Option<SshTransportGenerationV1>,
        connection: TmuxControlConnection,
    ) {
        self.control.send_replace(Some((transport, connection)));
    }

    fn command_client(
        &self,
        client: TmuxClient,
        transport: Option<&SshTransportGenerationV1>,
    ) -> TmuxClient {
        let control = self.control.borrow();
        match &*control {
            Some((generation, connection))
                if generation.as_ref() == transport && !connection.is_closed() =>
            {
                client.with_control_connection(connection.clone())
            }
            _ => client,
        }
    }

    /// Background terminal size activation must share the target control channel,
    /// not compete with discovery for another SSH session. The caller bounds and
    /// cancels this wait; no connection or observation is started here.
    pub(crate) async fn wait_for_control_client(
        &self,
        transport: Option<&SshTransportGenerationV1>,
    ) -> Result<TmuxClient> {
        let mut control = self.control.subscribe();
        loop {
            {
                let slot = control.borrow_and_update();
                match &*slot {
                    Some((generation, connection))
                        if generation.as_ref() == transport && !connection.is_closed() =>
                    {
                        return Ok(self
                            .client
                            .clone()
                            .with_control_connection(connection.clone()));
                    }
                    _ => {}
                }
            }
            control
                .changed()
                .await
                .context("target control slot closed")?;
        }
    }

    pub(crate) fn ensure_resource_supervisor(
        &self,
        ssh: &SshConnectionManager,
    ) -> Result<(), SshConnectionError> {
        if let TmuxTarget::Ssh { destination } = self.client.target() {
            self.ensure_supervisor(ssh, destination)?;
        }
        Ok(())
    }

    pub async fn resource_ssh_lease(
        &self,
        ssh_connections: &SshConnectionManager,
    ) -> Result<Option<SshConnectionLease>, SshConnectionError> {
        match self.client.target() {
            TmuxTarget::Ssh { destination } => {
                self.ensure_supervisor(ssh_connections, destination)?;
                let lease = self
                    .current_or_acquire_lease(ssh_connections, destination)
                    .await?;
                Ok(Some(lease))
            }
            TmuxTarget::Local | TmuxTarget::Wsl { .. } | TmuxTarget::MultiplexedSsh { .. } => {
                Ok(None)
            }
        }
    }

    pub async fn managed_client_for_attach(
        &self,
        ssh_connections: &SshConnectionManager,
    ) -> Result<ManagedAttachClient, SshConnectionError> {
        match self.client.target() {
            TmuxTarget::Ssh { destination } => {
                self.ensure_supervisor(ssh_connections, destination)?;
                ssh_connections.record_consumer_recovery_stage("pane_supervisor_ready");
                let lease = match ssh_connections
                    .lease_if_connected(&self.id, destination, self.route_profile.as_ref())
                    .await
                {
                    Ok(lease) => lease,
                    Err(_) => {
                        self.wait_for_connected_after(ssh_connections, destination, None)
                            .await?;
                        ssh_connections
                            .lease_if_connected(&self.id, destination, self.route_profile.as_ref())
                            .await?
                    }
                };
                let transport_generation =
                    Some(ssh_connections.transport_generation_for_lease(&lease));
                Ok(ManagedAttachClient {
                    client: self.client.clone().with_target(lease.tmux_target()?),
                    transport_generation,
                })
            }
            TmuxTarget::Local | TmuxTarget::Wsl { .. } | TmuxTarget::MultiplexedSsh { .. } => {
                Ok(ManagedAttachClient {
                    client: self.client.clone(),
                    transport_generation: None,
                })
            }
        }
    }

    async fn current_or_acquire_lease(
        &self,
        ssh_connections: &SshConnectionManager,
        destination: &str,
    ) -> Result<SshConnectionLease, SshConnectionError> {
        if let Ok(lease) = ssh_connections
            .lease_if_connected(&self.id, destination, self.route_profile.as_ref())
            .await
        {
            return Ok(lease);
        }
        match &self.route_profile {
            Some(profile) => {
                ssh_connections
                    .acquire_routed(&self.id, destination, profile)
                    .await
            }
            None => ssh_connections.acquire(&self.id, destination).await,
        }
    }

    pub async fn wait_for_connected_after(
        &self,
        ssh_connections: &SshConnectionManager,
        destination: &str,
        after: Option<SshTransportGenerationV1>,
    ) -> Result<SshTransportGenerationV1, SshConnectionError> {
        match &self.route_profile {
            Some(profile) => {
                ssh_connections
                    .wait_for_connected_after_routed(&self.id, destination, profile, after)
                    .await
            }
            None => {
                ssh_connections
                    .wait_for_connected_after(&self.id, destination, after)
                    .await
            }
        }
    }

    fn ensure_supervisor(
        &self,
        ssh_connections: &SshConnectionManager,
        destination: &str,
    ) -> Result<(), SshConnectionError> {
        match &self.route_profile {
            Some(profile) => {
                ssh_connections.ensure_supervisor_routed(&self.id, destination, profile.clone())
            }
            None => ssh_connections.ensure_supervisor(&self.id, destination),
        }
    }

    /// Configuration identity only; SSH execution requires a verified lease.
    pub(crate) fn configured_target(&self) -> &TmuxTarget { self.client.target() }

    pub(crate) fn configured_client(&self) -> &TmuxClient {
        &self.client
    }

    pub fn descriptor(&self) -> ExecutionTargetDescriptor {
        ExecutionTargetDescriptor {
            id: ExecutionTarget::new(self.id.clone()).expect("registered target IDs are validated"),
            display_name: self.display_name.clone(),
            kind: self.kind,
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    /// Stable while only tmux topology changes; replaced with target configuration.
    pub(crate) fn configuration_revision(&self) -> u64 {
        self.configuration_revision
    }

    pub fn advance_revision(&self) -> u64 {
        self.revision.fetch_add(1, Ordering::AcqRel) + 1
    }

    pub fn operation_gate(&self) -> Arc<RwLock<()>> {
        Arc::clone(&self.operation_gate)
    }

    /// Record a privacy-preserving topology observation and advance the
    /// revision exactly when an already-initialized observation changed.
    pub fn record_topology_observation(&self, topology: TmuxTopologySnapshot) -> u64 {
        let mut observed = self
            .observed_topology
            .lock()
            .expect("target topology observation lock poisoned");
        match &*observed {
            TopologyObservation::Uninitialized => {
                *observed = TopologyObservation::Known(topology);
                self.revision()
            }
            TopologyObservation::Known(previous) if previous == &topology => self.revision(),
            TopologyObservation::Known(_) | TopologyObservation::UnknownAfterMutation => {
                *observed = TopologyObservation::Known(topology);
                self.advance_revision()
            }
        }
    }

    /// Advance for a successful Host mutation whose state is not represented
    /// by the topology projection, while aligning the next observation when
    /// one is available.
    pub fn record_mutation(&self, topology: Option<TmuxTopologySnapshot>) -> u64 {
        *self
            .observed_topology
            .lock()
            .expect("target topology observation lock poisoned") = match topology {
            Some(topology) => TopologyObservation::Known(topology),
            None => TopologyObservation::UnknownAfterMutation,
        };
        self.advance_revision()
    }

    /// Replace an unknown post-mutation observation without publishing
    /// another revision. The caller must still hold the target operation
    /// write guard for the mutation that already advanced the revision.
    pub fn align_published_mutation_topology(&self, topology: TmuxTopologySnapshot) {
        *self
            .observed_topology
            .lock()
            .expect("target topology observation lock poisoned") =
            TopologyObservation::Known(topology);
    }
}

#[derive(Debug, Clone)]
pub struct TargetRegistry {
    state: Arc<StdRwLock<TargetRegistryState>>,
    config_path: Option<Arc<PathBuf>>,
    update_gate: Arc<StdMutex<()>>,
    next_configuration_revision: Arc<AtomicU64>,
}

#[derive(Debug)]
struct TargetRegistryState {
    polar_targets: Vec<crate::polar_target_config::PolarTargetConfig>,
    targets: HashMap<String, RegisteredTarget>,
    configured_targets: Vec<ConfiguredTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceSshTarget {
    pub(crate) target_id: String,
    pub(crate) revision: u64,
    pub(crate) destination: String,
    pub(crate) route_profile: Option<SshRouteProfile>,
}

impl WorkspaceSshTarget {
    pub(crate) fn route_profile_projection(&self) -> Option<SshRouteProfileV1> {
        self.route_profile.as_ref().map(route_profile_to_protocol)
    }

    pub(crate) fn ensure_supervisor(
        &self,
        ssh_connections: &SshConnectionManager,
    ) -> Result<(), SshConnectionError> {
        match &self.route_profile {
            Some(profile) => ssh_connections.ensure_supervisor_routed(
                &self.target_id,
                &self.destination,
                profile.clone(),
            ),
            None => ssh_connections.ensure_supervisor(&self.target_id, &self.destination),
        }
    }
}

impl TargetRegistry {
    fn allocate_configuration_revision(&self) -> Result<u64> {
        self.next_configuration_revision
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| value.checked_add(1))
            .map_err(|_| anyhow::anyhow!("target configuration generation exhausted"))
    }

    pub fn local(client: TmuxClient) -> Self {
        let local = RegisteredTarget {
            id: LOCAL_TARGET_ID.to_string(),
            display_name: "This computer".to_string(),
            kind: ExecutionTargetKind::Local,
            client,
            route_profile: None,
            revision: Arc::new(AtomicU64::new(1)),
            configuration_revision: 1,
            operation_gate: Arc::new(RwLock::new(())),
            observed_topology: Arc::new(StdMutex::new(TopologyObservation::default())),
            control: watch::channel(None).0,
        };
        Self {
            state: Arc::new(StdRwLock::new(TargetRegistryState {
                targets: HashMap::from([(LOCAL_TARGET_ID.to_string(), local)]),
                configured_targets: Vec::new(),
                polar_targets: Vec::new(),
            })),
            config_path: None,
            update_gate: Arc::new(StdMutex::new(())),
            next_configuration_revision: Arc::new(AtomicU64::new(2)),
        }
    }

    pub fn load(local_client: TmuxClient, path: Option<&Path>) -> Result<Self> {
        let local = Self::local(local_client);
        let mut targets = local
            .state
            .read()
            .expect("target registry lock poisoned")
            .targets
            .clone();
        let Some(path) = path else {
            return Ok(local);
        };
        let config = if path.exists() {
            let bytes = read_validated_target_config(path)?;
            serde_json::from_slice(&bytes)
                .with_context(|| format!("failed to decode target registry {}", path.display()))?
        } else {
            TargetRegistryFile::default()
        };

        crate::polar_target_config::PolarTargetConfig::validate_all(&config.polar_targets)?;
        for route in &config.polar_targets {
            if let crate::polar_target_config::PolarRoute::SshUds {execution_target_id, ..} = &route.route {
                anyhow::ensure!(config.targets.iter().any(|t| t.id() == execution_target_id),
                    "Polar route references an unconfigured SSH execution target");
            }
        }
        for configured in &config.targets {
            let id = configured.id().to_string();
            validate_target_id(&id)?;
            if targets.contains_key(&id) {
                bail!("duplicate Polar Tmux execution target ID {id:?}");
            }
            let route_profile = configured.route_profile().cloned();
            let (client, display_name, kind) = configured.clone().into_client()?;
            targets.insert(
                id.clone(),
                RegisteredTarget {
                    id,
                    display_name,
                    kind,
                    client,
                    route_profile,
                    revision: Arc::new(AtomicU64::new(1)),
                    configuration_revision: 1,
                    operation_gate: Arc::new(RwLock::new(())),
                    observed_topology: Arc::new(StdMutex::new(TopologyObservation::default())),
                    control: watch::channel(None).0,
                },
            );
        }

        Ok(Self {
            state: Arc::new(StdRwLock::new(TargetRegistryState {
                targets,
                configured_targets: config.targets,
                polar_targets: config.polar_targets,
            })),
            config_path: Some(Arc::new(path.to_path_buf())),
            update_gate: Arc::new(StdMutex::new(())),
            next_configuration_revision: Arc::new(AtomicU64::new(2)),
        })
    }

    pub(crate) fn polar_targets(&self) -> Vec<crate::polar_target_config::PolarTargetConfig> {
        self.state.read().expect("target registry lock poisoned").polar_targets.clone()
    }

    pub fn get(&self, id: &str) -> Option<RegisteredTarget> {
        self.state
            .read()
            .expect("target registry lock poisoned")
            .targets
            .get(id)
            .cloned()
    }

    

    

    pub fn descriptors(&self) -> Vec<ExecutionTargetDescriptor> {
        let state = self.state.read().expect("target registry lock poisoned");
        let mut descriptors = state
            .targets
            .values()
            .map(RegisteredTarget::descriptor)
            .collect::<Vec<_>>();
        descriptors.sort_by(|left, right| left.id.cmp(&right.id));
        descriptors
    }

    pub fn has_remote_target(&self) -> bool {
        self.state
            .read()
            .expect("target registry lock poisoned")
            .targets
            .values()
            .any(|target| target.kind == ExecutionTargetKind::Ssh)
    }

    pub(crate) fn ssh_targets(&self) -> Vec<WorkspaceSshTarget> {
        let state = self.state.read().expect("target registry lock poisoned");
        state
            .configured_targets
            .iter()
            .map(|target| WorkspaceSshTarget {
                target_id: target.id().to_owned(),
                revision: state
                    .targets
                    .get(target.id())
                    .map(RegisteredTarget::revision)
                    .unwrap_or_default(),
                destination: target.destination().to_owned(),
                route_profile: target.route_profile().cloned(),
            })
            .collect()
    }

    pub(crate) fn configure_ssh_target(
        &self,
        target_id: &str,
        expected_revision: u64,
        destination: &str,
        route_profile: Option<&SshRouteProfileV1>,
    ) -> Result<WorkspaceSshTarget> {
        if !is_canonical_ssh_destination(destination) {
            bail!("SSH execution target destination is invalid");
        }
        let route_profile = route_profile.map(route_profile_from_protocol).transpose()?;
        validate_target_id(target_id)?;
        let path = self
            .config_path
            .as_deref()
            .context("target registry is not configured for persistence")?;
        let _update_guard = self
            .update_gate
            .lock()
            .expect("target registry update lock poisoned");
        let (next_config, next_revision) = {
            let state = self.state.read().expect("target registry lock poisoned");
            let current = state.targets.get(target_id);
            let current_revision = current.map_or(0, RegisteredTarget::revision);
            if current_revision != expected_revision {
                bail!("SSH target revision is stale");
            }
            let mut configured_targets = state.configured_targets.clone();
            let index = configured_targets
                .iter()
                .position(|target| target.id() == target_id);
            let replacement = ConfiguredTarget::Ssh {
                id: target_id.to_owned(),
                display_name: index
                    .and_then(|index| configured_targets[index].display_name().cloned())
                    .or_else(|| Some(target_id.to_owned())),
                destination: destination.to_owned(),
                socket_name: index
                    .and_then(|index| configured_targets[index].socket_name().cloned()),
                workspace_id: index
                    .and_then(|index| configured_targets[index].workspace_id().map(str::to_owned)),
                route_profile: route_profile.clone(),
            };
            if let Some(index) = index {
                configured_targets[index] = replacement;
            } else {
                configured_targets.push(replacement);
            }
            (
                TargetRegistryFile {
                    polar_targets: state.polar_targets.clone(),
                    targets: configured_targets,
                },
                current_revision.saturating_add(1).max(1),
            )
        };

        write_private_target_config(path, &next_config)?;
        let current_state = self.state.read().expect("target registry lock poisoned");
        let mut replacement_targets = HashMap::new();
        for configured in &next_config.targets {
            let id = configured.id().to_owned();
            let unchanged = current_state
                .configured_targets
                .iter()
                .find(|current| current.id() == id)
                .is_some_and(|current| current == configured);
            if unchanged {
                if let Some(target) = current_state.targets.get(&id) {
                    replacement_targets.insert(id, target.clone());
                    continue;
                }
            }
            let route_profile = configured.route_profile().cloned();
            let (client, display_name, kind) = configured.clone().into_client()?;
            let revision = if id == target_id {
                next_revision
            } else {
                current_state
                    .targets
                    .get(&id)
                    .map(RegisteredTarget::revision)
                    .unwrap_or(1)
            };
            replacement_targets.insert(
                id.clone(),
                RegisteredTarget {
                    id,
                    display_name,
                    kind,
                    client,
                    route_profile,
                    revision: Arc::new(AtomicU64::new(revision)),
                    configuration_revision: self.allocate_configuration_revision()?,
                    operation_gate: Arc::new(RwLock::new(())),
                    observed_topology: Arc::new(StdMutex::new(TopologyObservation::default())),
                    control: watch::channel(None).0,
                },
            );
        }
        let local = current_state
            .targets
            .get(LOCAL_TARGET_ID)
            .cloned()
            .expect("target registry always contains local");
        drop(current_state);
        replacement_targets.insert(LOCAL_TARGET_ID.to_owned(), local);
        {
            let mut state = self.state.write().expect("target registry lock poisoned");
            state.targets = replacement_targets;
            state.configured_targets = next_config.targets;
        }
        Ok(WorkspaceSshTarget {
            target_id: target_id.to_owned(),
            revision: next_revision,
            destination: destination.to_owned(),
            route_profile,
        })
    }
}

fn route_profile_from_protocol(profile: &SshRouteProfileV1) -> Result<SshRouteProfile> {
    profile
        .validate()
        .map_err(|_| anyhow::anyhow!("SSH execution target route profile is invalid"))?;
    let doh_providers = profile
        .doh_providers
        .iter()
        .map(|provider| match provider.as_str() {
            "google" => Ok(DohProvider::Google),
            "cloudflare" => Ok(DohProvider::Cloudflare),
            _ => bail!("SSH execution target route profile has an unsupported DoH provider"),
        })
        .collect::<Result<Vec<_>>>()?;
    let route_profile = SshRouteProfile {
        lan_address: profile
            .lan_address
            .parse::<Ipv4Addr>()
            .context("SSH execution target LAN address is invalid")?,
        lan_port: profile.lan_port,
        wan_host: profile.wan_host.clone(),
        wan_port: profile.wan_port,
        doh_providers,
    };
    if !route_profile.validate() {
        bail!("SSH execution target route profile is invalid");
    }
    Ok(route_profile)
}

fn route_profile_to_protocol(profile: &SshRouteProfile) -> SshRouteProfileV1 {
    SshRouteProfileV1 {
        lan_address: profile.lan_address.to_string(),
        lan_port: profile.lan_port,
        wan_host: profile.wan_host.clone(),
        wan_port: profile.wan_port,
        doh_providers: profile
            .doh_providers
            .iter()
            .map(|provider| match provider {
                DohProvider::Google => "google".to_owned(),
                DohProvider::Cloudflare => "cloudflare".to_owned(),
            })
            .collect(),
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetRegistryFile {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    polar_targets: Vec<crate::polar_target_config::PolarTargetConfig>,
    #[serde(default)]
    targets: Vec<ConfiguredTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ConfiguredTarget {
    Ssh {
        id: String,
        #[serde(default)]
        display_name: Option<String>,
        destination: String,
        #[serde(default)]
        socket_name: Option<String>,
        #[serde(default)]
        workspace_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        route_profile: Option<SshRouteProfile>,
    },
}

impl ConfiguredTarget {
    fn id(&self) -> &str {
        match self {
            Self::Ssh { id, .. } => id,
        }
    }

    fn destination(&self) -> &str {
        match self {
            Self::Ssh { destination, .. } => destination,
        }
    }

    fn display_name(&self) -> Option<&String> {
        match self {
            Self::Ssh { display_name, .. } => display_name.as_ref(),
        }
    }

    fn workspace_id(&self) -> Option<&str> {
        match self {
            Self::Ssh { workspace_id, .. } => workspace_id.as_deref(),
        }
    }

    fn socket_name(&self) -> Option<&String> {
        match self {
            Self::Ssh { socket_name, .. } => socket_name.as_ref(),
        }
    }

    fn route_profile(&self) -> Option<&SshRouteProfile> {
        match self {
            Self::Ssh { route_profile, .. } => route_profile.as_ref(),
        }
    }

    fn into_client(self) -> Result<(TmuxClient, String, ExecutionTargetKind)> {
        match self {
            Self::Ssh {
                id,
                display_name,
                destination,
                socket_name,
                workspace_id: _,
                route_profile,
            } => {
                if !is_canonical_ssh_destination(&destination) {
                    bail!("SSH execution target destination is invalid");
                }
                let client = TmuxClient::new(TmuxTarget::ssh(destination));
                let display_name = display_name.unwrap_or_else(|| id.clone());
                validate_display_name(&display_name)?;
                if route_profile
                    .as_ref()
                    .is_some_and(|profile| !profile.validate())
                {
                    bail!("SSH execution target route profile is invalid");
                }
                let client = match socket_name {
                    Some(socket_name) => {
                        validate_socket_name(&socket_name)?;
                        client.with_socket_name(socket_name)
                    }
                    None => client,
                };
                Ok((client, display_name, ExecutionTargetKind::Ssh))
            }
        }
    }
}

fn validate_target_id(id: &str) -> Result<()> {
    if id == LOCAL_TARGET_ID {
        bail!("{LOCAL_TARGET_ID:?} is reserved for the local execution target");
    }
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("execution target ID must contain 1-64 ASCII letters, digits, - or _");
    }
    Ok(())
}

fn validate_socket_name(name: &str) -> Result<()> {
    if !is_canonical_tmux_socket_name(name) {
        bail!("tmux socket name must match [A-Za-z0-9][A-Za-z0-9._-]{{0,95}}");
    }
    Ok(())
}

fn validate_display_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 128
        || name != name.trim()
        || name.chars().any(char::is_control)
    {
        bail!("execution target display name is invalid");
    }
    Ok(())
}

fn write_private_target_config(path: &Path, config: &TargetRegistryFile) -> Result<()> {
    let parent = path
        .parent()
        .context("target registry path has no parent directory")?;
    let parent_existed = parent.exists();
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create target registry directory {}",
            parent.display()
        )
    })?;
    #[cfg(unix)]
    if !parent_existed {
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).with_context(|| {
            format!(
                "failed to protect target registry directory {}",
                parent.display()
            )
        })?;
    }
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.file_type().is_file() {
            bail!("target registry must be a regular file, not a symlink or special file");
        }
    }

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("target registry file name is invalid")?;
    let temporary_path = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let result = (|| -> Result<()> {
        let mut file = options.open(&temporary_path).with_context(|| {
            format!(
                "failed to create private target registry {}",
                temporary_path.display()
            )
        })?;
        let bytes =
            serde_json::to_vec_pretty(config).context("failed to encode target registry")?;
        file.write_all(&bytes)
            .context("failed to write target registry")?;
        file.write_all(b"\n")
            .context("failed to finish target registry")?;
        file.sync_all().context("failed to sync target registry")?;
        fs::rename(&temporary_path, path)
            .with_context(|| format!("failed to replace target registry {}", path.display()))?;
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .context("failed to protect target registry")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

fn read_validated_target_config(path: &Path) -> Result<Vec<u8>> {
    let mut file = open_validated_target_config(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .with_context(|| format!("failed to read target registry {}", path.display()))?;
    Ok(bytes)
}

fn open_validated_target_config(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);

    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) => {
            #[cfg(unix)]
            if error.raw_os_error() == Some(libc::ELOOP) {
                bail!("target registry must be a regular file, not a symlink or special file");
            }
            return Err(error)
                .with_context(|| format!("failed to open target registry {}", path.display()));
        }
    };
    let metadata = file
        .metadata()
        .with_context(|| format!("failed to inspect target registry {}", path.display()))?;
    if !metadata.file_type().is_file() {
        bail!("target registry must be a regular file, not a symlink or special file");
    }
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no arguments and is thread-safe on supported
        // Unix platforms.
        let effective_uid = unsafe { libc::geteuid() };
        if metadata.uid() != effective_uid {
            bail!("target registry must be owned by the Host user");
        }
        if metadata.permissions().mode() & 0o022 != 0 {
            bail!("target registry must not be writable by group or other users");
        }
    }
    Ok(file)
}


