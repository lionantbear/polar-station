use super::*;
use crate::polar_target_config::{PolarRoute, PolarTargetConfig};
use polar_tmux_protocol::{ConnectionChangeV1, ConnectionsProjectionV1, MachineConnectionV1, PolarConnectionV1};
use sha2::{Digest, Sha256};

fn revision(config: &TargetRegistryFile) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(config).expect("target config serializable")))
}

impl TargetRegistry {
    pub(crate) fn connections(&self) -> ConnectionsProjectionV1 {
        let state = self.state.read().expect("target registry lock poisoned");
        let config = TargetRegistryFile { targets: state.configured_targets.clone(), polar_targets: state.polar_targets.clone() };
        ConnectionsProjectionV1 {
            revision: revision(&config), last_check: None, machine_statuses: Vec::new(),
            machines: config.targets.iter().map(|t| MachineConnectionV1 {
                id: t.id().into(), display_name: t.display_name().cloned().unwrap_or_else(|| t.id().into()),
                destination: t.destination().into(), route_profile: t.route_profile().map(route_profile_to_protocol),
            }).collect(),
            polar_instances: config.polar_targets.iter().map(|t| {
                let (execution_target_id, socket_path, expected_peer_uid) = match &t.route {
                    PolarRoute::LocalUds { socket_path, expected_peer_uid } => (LOCAL_TARGET_ID.into(), socket_path, *expected_peer_uid),
                    PolarRoute::SshUds { execution_target_id, remote_socket_path } => (execution_target_id.clone(), remote_socket_path, None),
                };
                PolarConnectionV1 { id: t.id.clone(), display_name: t.display_name.clone(), expected_node_id: t.expected_node_id.clone(),
                    execution_target_id, socket_path: socket_path.to_string_lossy().into_owned(), expected_peer_uid }
            }).collect(),
        }
    }

    /// Commit a single form against the exact catalog reviewed by its window.
    /// Session topology revisions deliberately do not invalidate configuration drafts.
    pub(crate) fn update_connection(&self, expected: &str, change: ConnectionChangeV1) -> Result<()> {
        let path = self.config_path.as_deref().context("Connections cannot be saved without a target configuration path")?;
        let _guard = self.update_gate.lock().expect("target update lock poisoned");
        let mut state = self.state.write().expect("target registry lock poisoned");
        let mut config = TargetRegistryFile { targets: state.configured_targets.clone(), polar_targets: state.polar_targets.clone() };
        anyhow::ensure!(revision(&config) == expected, "Connections changed in another window. Reload before saving.");
        match change {
            ConnectionChangeV1::SaveMachine { mut machine } => {
                if machine.id.is_empty() { machine.id = format!("machine-{}", crate::host_identity::random_uuid_v4()); }
                validate_target_id(&machine.id)?;
                let previous = config.targets.iter().find(|t| t.id() == machine.id);
                let next = ConfiguredTarget::Ssh {
                    id: machine.id.clone(), display_name: Some(machine.display_name), destination: machine.destination,
                    socket_name: previous.and_then(|t| t.socket_name().cloned()),
                    workspace_id: previous.and_then(|t| t.workspace_id().map(str::to_owned)),
                    route_profile: machine.route_profile.as_ref().map(route_profile_from_protocol).transpose()?,
                };
                next.clone().into_client()?;
                if let Some(t) = config.targets.iter_mut().find(|t| t.id() == machine.id) { *t = next; } else { config.targets.push(next); }
            }
            ConnectionChangeV1::SavePolar { mut polar } => {
                if polar.id.is_empty() { polar.id = format!("polar-{}", crate::host_identity::random_uuid_v4()); }
                let route = if polar.execution_target_id == LOCAL_TARGET_ID {
                    PolarRoute::LocalUds { socket_path: polar.socket_path.into(), expected_peer_uid: polar.expected_peer_uid }
                } else {
                    anyhow::ensure!(polar.expected_peer_uid.is_none(), "Remote UID is determined by the SSH authenticated account");
                    PolarRoute::SshUds { execution_target_id: polar.execution_target_id, remote_socket_path: polar.socket_path.into() }
                };
                let terminal_targets = config.polar_targets.iter().find(|t| t.id == polar.id).map(|t|t.terminal_targets.clone()).unwrap_or_default();
                let next = PolarTargetConfig { terminal_targets, id: polar.id, display_name: polar.display_name, expected_node_id: polar.expected_node_id, route };
                if let Some(t) = config.polar_targets.iter_mut().find(|t| t.id == next.id) { *t = next; } else { config.polar_targets.push(next); }
            }
            ConnectionChangeV1::RemoveMachine { id } => {
                validate_target_id(&id)?;
                anyhow::ensure!(config.targets.iter().any(|t| t.id() == id), "Machine no longer exists");
                config.targets.retain(|t| t.id() != id);
            }
            ConnectionChangeV1::RemovePolar { id } => {
                anyhow::ensure!(config.polar_targets.iter().any(|t| t.id == id), "Polar instance no longer exists");
                config.polar_targets.retain(|t| t.id != id);
            }
        }
        PolarTargetConfig::validate_all(&config.polar_targets)?;
        for polar in &config.polar_targets {
            if let PolarRoute::SshUds { execution_target_id, .. } = &polar.route {
                anyhow::ensure!(config.targets.iter().any(|t| t.id() == execution_target_id), "Polar instance '{}' references machine '{}'; edit or remove that instance first", polar.display_name, execution_target_id);
            }
        }
        let mut targets = HashMap::new();
        targets.insert(LOCAL_TARGET_ID.into(), state.targets[LOCAL_TARGET_ID].clone());
        for t in &config.targets {
            let (client, display_name, kind) = t.clone().into_client()?;
            let previous_config = state.configured_targets.iter().find(|v| v.id() == t.id());
            let unchanged_route = previous_config.is_some_and(|old| old.destination() == t.destination() && old.route_profile() == t.route_profile() && old.socket_name() == t.socket_name());
            let target = if let Some(old) = state.targets.get(t.id()).filter(|_| unchanged_route) {
                let mut next = old.clone(); next.display_name = display_name; next
            } else {
                RegisteredTarget { id: t.id().into(), display_name, kind, client, route_profile: t.route_profile().cloned(),
                    revision: Arc::new(AtomicU64::new(state.targets.get(t.id()).map_or(1, |v| v.revision().saturating_add(1)))),
                    configuration_revision: self.allocate_configuration_revision()?,
                    operation_gate: Arc::new(RwLock::new(())), observed_topology: Arc::new(StdMutex::new(TopologyObservation::default())), control: watch::channel(None).0 }
            };
            targets.insert(t.id().into(), target);
        }
        write_private_target_config(path, &config)?;
        state.targets = targets;
        state.configured_targets = config.targets;
        state.polar_targets = config.polar_targets;
        Ok(())
    }
}
