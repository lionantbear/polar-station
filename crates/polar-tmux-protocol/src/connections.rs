//! Native Connections forms carry configuration; Host owns validation and persistence.
use serde::{Deserialize, Serialize};
use crate::SshRouteProfileV1;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionsProjectionV1 {
    pub revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_check: Option<ConnectionCheckV1>,
    #[serde(default)]
    pub machine_statuses: Vec<MachineConnectionStatusV1>,
    pub machines: Vec<MachineConnectionV1>,
    pub polar_instances: Vec<PolarConnectionV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineConnectionV1 {
    pub id: String,
    pub display_name: String,
    pub destination: String,
    pub route_profile: Option<SshRouteProfileV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolarConnectionV1 {
    pub id: String,
    pub display_name: String,
    pub execution_target_id: String,
    pub socket_path: String,
    pub expected_node_id: String,
    pub expected_peer_uid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConnectionChangeV1 {
    SaveMachine { machine: MachineConnectionV1 },
    SavePolar { polar: PolarConnectionV1 },
    RemoveMachine { id: String },
    RemovePolar { id: String },
}

/// A correlated read result, never persisted with target configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionCheckV1 {
    pub request_id: String,
    pub target_id: String,
    pub polar: bool,
    pub ready: bool,
    pub summary: String,
}

/// Live SSH observation, separate from persisted machine configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineConnectionStatusV1 {
    pub id: String,
    pub connection: crate::ControlConnectionProjectionV1,
}
