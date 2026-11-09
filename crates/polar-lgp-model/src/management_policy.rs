use polar_identity::UnixTimestamp;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagementTransportPolicyRecord {
    pub plane: String,
    pub require_listener_isolation: bool,
    pub default_required_access_plane: String,
    pub updated_at: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagementActorClassRecord {
    pub class_id: String,
    pub allowed_system_role: String,
    pub updated_at: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagementToolPolicyRecord {
    pub tool_family: String,
    pub management_only: bool,
    pub allowed_actor_class: String,
    pub allowed_thread_class: String,
    pub updated_at: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagementChannelPolicyRecord {
    pub channel_id: String,
    pub management_class: String,
    pub plane: String,
    pub allowed_actor_class: String,
    pub updated_at: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagementThreadPolicyRecord {
    pub channel_id: String,
    pub thread_id: String,
    pub management_class: String,
    pub allowed_tool_family: String,
    pub updated_at: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagementConnectorPolicyRecord {
    pub connector: String,
    pub default_intake_channel_id: String,
    pub default_intake_thread_id: String,
    pub event_class: String,
    pub updated_at: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManagementPolicyRecordSet {
    pub transport: Option<ManagementTransportPolicyRecord>,
    pub actor_classes: Vec<ManagementActorClassRecord>,
    pub tool_policies: Vec<ManagementToolPolicyRecord>,
    pub channels: Vec<ManagementChannelPolicyRecord>,
    pub threads: Vec<ManagementThreadPolicyRecord>,
    pub connectors: Vec<ManagementConnectorPolicyRecord>,
}
